//! The Blender link: Send to Blender, and what Blender sends back.
//!
//! One press sends the selected objects to Blender as they stand, at their
//! real size, and opens Blender if it is not open. In Blender, the GolemSculpt
//! tab of the sidebar has the button for the way back: an object that came
//! from here returns as itself, its shape replaced in one undo step, and
//! anything made there arrives as a new object.
//!
//! How the two programs meet: a folder both know, `blender_link` in the
//! settings folder. Whichever sends writes one package (`io/link.rs`) into
//! the other's tray there, and the other looks in its tray twice a second.
//! Nothing listens on a port. Blender's side of it is one script,
//! `assets/blender/golemsculpt_link.py`, which is part of this program: it is
//! written into the folder and Blender is started with it, and from then on
//! it keeps itself in Blender's own startup folder, so the link is there
//! however Blender is opened.
//!
//! What comes back is put where it stands in Blender, whatever was done to
//! it there (moved, turned, resized, its transforms applied): the points
//! arrive in the scene's own space and are carried into the object's. The
//! object itself is left as it was: its centre, turn and size, its symmetry,
//! its live mirror, its instances. So a symmetric sculpt is still sculpted
//! about the same planes. If the model was moved a long way in Blender, Set
//! Origin to Mesh brings its centre after it.
//!
//! Only what was worked on is replaced. Each sending of an object has a
//! mark; both sides remember the mark of what they last exchanged for it and
//! what it was like then, and one that has not changed since says so
//! (`same_as`). The side that receives that leaves its own object alone: it
//! is at least as new. So a whole selection can go back and forth, and a mask,
//! hidden faces or the quads of a quad remesh are lost only on an object
//! Blender really changed. (A mesh from Blender is a new mesh: it has none.)
//! Changed or not, an object also goes with the mark of what was last
//! exchanged for it (`was`), and since no two marks are alike that says
//! exactly which object it is on the other side, whatever each program has
//! come to call it: a block made in Blender and named `Cube` there is still
//! the same block when this scene, which had a `Cube`, calls it `Cube (2)`.
//!
//! Paint goes too, as Blender colour attributes, and what is painted in
//! Blender comes back as a paint layer: `blender/paint.rs`.
//!
//! A scripted run has no link to the user's Blender: it has no settings
//! folder (or one of its own), it starts no Blender and it opens no file box.

use std::collections::HashMap;
use std::hash::Hasher;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use glam::{Mat4, Vec3};
use serde_json::{Value, json};
use winit::event_loop::EventLoopProxy;

use super::{Drag, Running};
use crate::bridge_guard::{off_limits, process_is_running};
use crate::io::{self, LINK_ENDING, LinkObject, LinkPackage};
use crate::mesh::Mesh;
use crate::prefs::{self, Home};
use crate::render::flash;
use crate::scene::{Object, Scene};

mod paint;

/// The folder both programs know, in the settings folder.
const FOLDER: &str = "blender_link";
const TO_BLENDER: &str = "to_blender";
const TO_SCULPT: &str = "to_sculpt";
/// Blender's side of the link, and the name it is written under.
const SCRIPT: &str = include_str!("../../assets/blender/golemsculpt_link.py");
const SCRIPT_NAME: &str = "golemsculpt_link.py";
/// How often the tray is looked at.
const LOOK: Duration = Duration::from_millis(500);
/// A package nobody took in this long was meant for a session that has gone.
const STALE: Duration = Duration::from_secs(600);
/// How long a Blender this app started is given to come up before another is started.
const OPENING: Duration = Duration::from_secs(120);

/// What was last exchanged with Blender for one place of an object.
struct Known {
    mark: String,
    /// The mesh and its place then, as [`print_of`] has them.
    print: u64,
}

/// What the app keeps for the link.
pub(super) struct State {
    /// The link's folder; `None` for a run with no settings folder.
    folder: Option<PathBuf>,
    /// This run's name, which begins every object's `ref` and every mark: the
    /// numbers objects go by here mean nothing to another run.
    run: String,
    marks: u64,
    /// By object and by which of its places (0 the object, then its copies).
    known: HashMap<(u64, usize), Known>,
    /// A Blender this run started that has not said it is up yet: its process, and when.
    opening: Option<(u32, Instant)>,
    /// A scripted run: it starts no Blender and puts up no file box.
    scripted: bool,
}

impl State {
    pub(super) fn new(screenshot: bool) -> State {
        let home = Home::for_run(screenshot);
        let folder = home.folder().filter(|_| home.writable()).map(|settings| settings.join(FOLDER));
        let since = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let run = format!("{:x}x{:x}", std::process::id(), since & 0xffff_ffff);
        State { folder, run, marks: 0, known: HashMap::new(), opening: None, scripted: prefs::scripted() }
    }

    fn fresh_mark(&mut self) -> String {
        self.marks += 1;
        format!("s{}-{}", self.run, self.marks)
    }
}

// ---- the tray, watched while the app runs

/// The watch on this app's tray: a thread that rouses the event loop, which
/// sleeps while nothing is happening, when Blender has left something.
pub struct Watch {
    arrived: Arc<AtomicBool>,
    /// The note that says this app is open, and where it is, for Blender's side.
    about: PathBuf,
}

/// Starts the watch. `settings` is this run's settings folder; a run without
/// one has no link. Also leaves the note Blender's side reads to tell whether
/// the app is open and to open it when it is not.
pub fn watch(wake: EventLoopProxy<()>, settings: Option<&Path>) -> Option<Watch> {
    let folder = settings?.join(FOLDER);
    let tray = folder.join(TO_SCULPT);
    if let Err(e) = std::fs::create_dir_all(&tray) {
        log::warn!("the Blender link is off ({}: {e})", tray.display());
        return None;
    }
    let about = folder.join("sculpt.json");
    let exe = std::env::current_exe().map(|exe| exe.to_string_lossy().into_owned()).unwrap_or_default();
    if let Err(e) = write_json(&about, &json!({ "pid": std::process::id(), "exe": exe })) {
        log::warn!("{e}");
    }
    let arrived = Arc::new(AtomicBool::new(false));
    let flag = arrived.clone();
    std::thread::spawn(move || watch_tray(&tray, &flag, || wake.send_event(()).is_ok()));
    Some(Watch { arrived, about })
}

/// The watch's thread: looks in the tray every [`LOOK`], and while a package
/// is waiting there says so and rouses the loop with `wake`. It says so again
/// at every look for as long as the package waits, since the app may be in the
/// middle of a stroke and leave it for later. Ends when `wake` says the loop
/// has gone.
fn watch_tray(tray: &Path, arrived: &AtomicBool, wake: impl Fn() -> bool) {
    loop {
        if !packages(tray).is_empty() {
            arrived.store(true, Ordering::Release);
            if !wake() {
                break;
            }
        }
        std::thread::sleep(LOOK);
    }
}

impl Watch {
    /// Takes in what has arrived, if anything has. Main thread only.
    pub fn pump(&self, app: &mut Running) {
        if self.arrived.swap(false, Ordering::AcqRel) {
            app.blender_arrivals();
        }
    }
}

impl Drop for Watch {
    /// The app is closing: it is no longer there to send to, and the note
    /// says so, but still says where the app is, so that Blender can open it.
    /// (A second copy of the app may have left its own note since; that one
    /// stays.)
    fn drop(&mut self) {
        let Some(about) = read_json(&self.about) else { return };
        if about["pid"].as_u64() == Some(std::process::id() as u64) {
            let _ = write_json(&self.about, &json!({ "pid": 0, "exe": about["exe"] }));
        }
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn write_json(path: &Path, value: &Value) -> io::Result<()> {
    let text = value.to_string();
    io::replace_file(path, |w| w.write_all(text.as_bytes()))
}

/// The packages waiting in a tray, oldest first (their names begin with the
/// time they were written). Anything else in there is a package still being
/// written, or one being read.
fn packages(tray: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(tray) else { return Vec::new() };
    let mut found: Vec<PathBuf> = entries
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.extension().is_some_and(|ending| ending == LINK_ENDING) && path.is_file())
        .collect();
    found.sort();
    found
}

fn older_than(path: &Path, age: Duration) -> bool {
    let written = std::fs::metadata(path).and_then(|about| about.modified());
    written.ok().and_then(|at| at.elapsed().ok()).is_some_and(|since| since > age)
}

/// Clears out of a tray what a program that closed part-way left in it.
fn sweep(tray: &Path) {
    let Ok(entries) = std::fs::read_dir(tray) else { return };
    for path in entries.filter_map(|entry| Some(entry.ok()?.path())) {
        if path.is_file() && older_than(&path, STALE) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

// ---- Blender: whether it is open, where it is, starting it

/// Whether a Blender with the link in it is open: each leaves a note with its
/// process number while it is. A note whose Blender has gone is cleared away.
fn blender_is_open(folder: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(folder) else { return false };
    let mut open = false;
    for path in entries.filter_map(|entry| Some(entry.ok()?.path())) {
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(pid) = name.strip_prefix("blender-").and_then(|rest| rest.strip_suffix(".json")) else { continue };
        match pid.parse::<u32>() {
            Ok(pid) if process_is_running(pid) => open = true,
            _ => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    open
}

/// The first version of Blender's side of the link that takes paint
/// (`LINK_VERSION` in the script).
const PAINT_LINK: u64 = 2;

/// Whether an open Blender has a copy of the link from before paint, which
/// takes the shapes of a package and passes its paint over: each Blender's
/// note says which version of the link it runs.
fn blender_link_is_old(folder: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(folder) else { return false };
    entries.filter_map(|entry| Some(entry.ok()?.path())).any(|path| {
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let running = name
            .strip_prefix("blender-")
            .and_then(|rest| rest.strip_suffix(".json"))
            .and_then(|pid| pid.parse::<u32>().ok())
            .is_some_and(process_is_running);
        running && read_json(&path).is_some_and(|about| about["link"].as_u64().unwrap_or(1) < PAINT_LINK)
    })
}

/// Whether any Blender at all is running, with the link or without: asked of
/// the system's own list of programs, with no window put up to ask it.
/// (Where that cannot be asked, the answer is no.)
fn any_blender_running() -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut list = Command::new("tasklist");
        list.args(["/FI", "IMAGENAME eq blender.exe", "/FO", "CSV", "/NH"]).stdin(Stdio::null()).creation_flags(CREATE_NO_WINDOW);
        list.output().is_ok_and(|said| String::from_utf8_lossy(&said.stdout).to_ascii_lowercase().contains("blender.exe"))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// A path to a program that this app may look at and run: on this computer's
/// own drives (asked of the text before the disk, as the Claude bridge asks:
/// `bridge_guard::off_limits`), and there.
fn usable(path: &Path) -> bool {
    off_limits(&path.to_string_lossy()).is_none() && path.is_file()
}

/// Where Blender is: where it last said it was (its side of the link leaves a
/// note of that every time it starts), else where `GOLEMSCULPT_BLENDER` says,
/// else where it is usually installed.
fn find_blender(folder: &Path) -> Option<PathBuf> {
    let noted = read_json(&folder.join("blender.json")).and_then(|about| Some(PathBuf::from(about["exe"].as_str()?)));
    let named = std::env::var_os("GOLEMSCULPT_BLENDER").filter(|path| !path.is_empty()).map(PathBuf::from);
    noted.into_iter().chain(named).chain(installed()).find(|exe| usable(exe))
}

/// The usual places, best first: Blender's own installer's folders, newest
/// version first; Steam's libraries; then wherever the system looks for programs.
fn installed() -> Vec<PathBuf> {
    let program = if cfg!(windows) { "blender.exe" } else { "blender" };
    let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from);
    let mut roots: Vec<PathBuf> = ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"].into_iter().filter_map(var).collect();
    roots.dedup();

    let mut found = Vec::new();
    for root in &roots {
        let Ok(entries) = std::fs::read_dir(root.join("Blender Foundation")) else { continue };
        let mut versions: Vec<(Vec<u32>, PathBuf)> = entries
            .filter_map(|entry| Some(entry.ok()?.path()))
            .map(|folder| (version_of(&folder.file_name().unwrap_or_default().to_string_lossy()), folder))
            .collect();
        versions.sort_by(|a, b| b.0.cmp(&a.0));
        found.extend(versions.into_iter().map(|(_, folder)| folder.join(program)));
    }
    for root in &roots {
        let Ok(list) = std::fs::read_to_string(root.join("Steam").join("steamapps").join("libraryfolders.vdf")) else { continue };
        found.extend(steam_libraries(&list).into_iter().map(|library| library.join("steamapps").join("common").join("Blender").join(program)));
    }
    if let Some(path) = std::env::var_os("PATH") {
        found.extend(std::env::split_paths(&path).map(|folder| folder.join(program)));
    }
    found
}

/// The numbers in a folder's name, for telling `Blender 4.2` from `Blender 4.10`.
fn version_of(name: &str) -> Vec<u32> {
    name.split(|c: char| !c.is_ascii_digit()).filter_map(|part| part.parse().ok()).collect()
}

/// The folders Steam installs into, out of its own list of them: every line
/// of the form `"path"  "D:\\SteamLibrary"`.
fn steam_libraries(list: &str) -> Vec<PathBuf> {
    let quoted = |line: &str| line.split('"').nth(3).map(|path| PathBuf::from(path.replace("\\\\", "\\")));
    list.lines().map(str::trim).filter(|line| line.starts_with("\"path\"")).filter_map(quoted).collect()
}

/// The program a chosen file stands for: Blender itself, also when it was its
/// launcher beside it that was picked. `None` for anything else.
fn chosen_blender(picked: &Path) -> Option<PathBuf> {
    let name = picked.file_stem()?.to_string_lossy().to_ascii_lowercase();
    match name.as_str() {
        "blender" => Some(picked.to_owned()),
        "blender-launcher" => Some(picked.with_file_name(if cfg!(windows) { "blender.exe" } else { "blender" })),
        _ => None,
    }
}

/// The command that starts Blender with the link's script: run directly, each
/// argument its own, with no console or input or output of this app's.
fn blender_command(exe: &Path, script: &Path) -> Command {
    let mut command = Command::new(exe);
    command.arg("--python").arg(script);
    if let Some(folder) = exe.parent().filter(|folder| !folder.as_os_str().is_empty()) {
        command.current_dir(folder);
    }
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    command
}

/// Writes Blender's side of the link into the folder, if the one there is not
/// this version's, and gives back where it is.
fn put_script(folder: &Path) -> io::Result<PathBuf> {
    let path = folder.join(SCRIPT_NAME);
    if std::fs::read(&path).is_ok_and(|there| there == SCRIPT.as_bytes()) {
        return Ok(path);
    }
    io::replace_file(&path, |w| w.write_all(SCRIPT.as_bytes()))?;
    Ok(path)
}

// ---- what goes, and what comes

/// A mesh and where it stands, and its paint, as one number: whether an
/// object is still what was last exchanged for it.
fn print_of(mesh: &Mesh, place: Mat4) -> u64 {
    let mut print = std::collections::hash_map::DefaultHasher::new();
    print.write(bytemuck::cast_slice(&mesh.positions));
    print.write(bytemuck::cast_slice(&mesh.indices));
    print.write(bytemuck::cast_slice(&place.to_cols_array()));
    print.write_u64(mesh.paint_print_by_place());
    print.finish()
}

/// Names as a status line lists them: `"Ball"`, `"Ball" and "Cube"`, and past
/// three of them `"Ball", "Cube" and 3 more`.
fn named(names: &[String]) -> String {
    let quoted = |name: &String| format!("\"{name}\"");
    match names {
        [] => String::new(),
        [only] => quoted(only),
        [first, second] => format!("{} and {}", quoted(first), quoted(second)),
        [first, second, third] => format!("{}, {} and {}", quoted(first), quoted(second), quoted(third)),
        [first, second, rest @ ..] => format!("{}, {} and {} more", quoted(first), quoted(second), rest.len()),
    }
}

/// What a press of Send to Blender writes.
struct Outgoing {
    objects: Vec<LinkObject>,
    /// For each of `objects`: whose place it is, and its print now.
    prints: Vec<((u64, usize), u64)>,
    /// The objects that go, by name (their copies go with them unnamed).
    names: Vec<String>,
    copies: usize,
    /// Selected objects with no surface, which are left out.
    bare: usize,
    /// Points written mended.
    mended: usize,
}

/// The selected objects as a package has them, in the list's order: each
/// where it stands, with its live mirror and array copies after it, which are
/// part of the model on screen, and each with its paint. `slot` is `Doc::mesh`.
fn outgoing(scene: &Scene, slot: &Mesh, link: &mut State, colours: &io::ExportPaint) -> Outgoing {
    let mut out = Outgoing { objects: Vec::new(), prints: Vec::new(), names: Vec::new(), copies: 0, bare: 0, mended: 0 };
    // Without the points and loose vertices dynamic topology's merges leave.
    let tidy = crate::scene::Tidied::of_selected(scene, slot);
    for object in scene.objects.iter().filter(|o| scene.is_selected(o.id)) {
        let Some(mesh) = tidy.mesh(scene, object.mesh, slot).filter(|mesh| !mesh.indices.is_empty()) else {
            out.bare += 1;
            continue;
        };
        out.names.push(object.name.clone());
        let painted = io::LinkPaint::of(mesh, colours);
        for (i, (copy_name, place)) in scene.export_places(object).into_iter().enumerate() {
            let (mut entry, mended) = LinkObject::placed(mesh, place);
            entry.paint = painted.clone();
            entry.name = if i == 0 { object.name.clone() } else { copy_name };
            entry.reference = format!("{}:{}", link.run, object.id);
            entry.copy = i > 0;
            let (key, print) = ((object.id, i), print_of(mesh, place));
            entry.was = link.known.get(&key).map(|known| known.mark.clone()).unwrap_or_default();
            match link.known.get(&key).filter(|known| known.print == print) {
                // As it was when it last went or came: Blender's own is left alone.
                Some(known) => (entry.mark, entry.same_as) = (known.mark.clone(), known.mark.clone()),
                None => entry.mark = link.fresh_mark(),
            }
            out.prints.push((key, print));
            out.objects.push(entry);
            out.copies += usize::from(i > 0);
            out.mended += if i == 0 { mended } else { 0 };
        }
    }
    out
}

/// The object of the scene a package's entry is, if it is one: the one its
/// `ref` names, when that was written by this run; else the one that was last
/// exchanged under the mark it says it was; else the one with the name it
/// went to Blender under. Never for a copy, which is not an object here.
fn target_of<'a>(scene: &'a Scene, link: &State, entry: &LinkObject) -> Option<&'a Object> {
    if entry.copy {
        return None;
    }
    let numbered =
        entry.reference.split_once(':').filter(|(theirs, _)| *theirs == link.run).and_then(|(_, id)| id.parse::<u64>().ok());
    let by_mark = || {
        let exchanged = |known: &Known| !entry.was.is_empty() && known.mark == entry.was;
        let id = link.known.iter().find(|(key, known)| key.1 == 0 && exchanged(known)).map(|(key, _)| key.0)?;
        scene.get(id)
    };
    let by_name = || scene.objects.iter().find(|o| !entry.sent_as.is_empty() && o.name == entry.sent_as);
    numbered.and_then(|id| scene.get(id)).or_else(by_mark).or_else(by_name)
}

/// What becomes of one entry of a package from Blender.
enum Fate {
    /// Its shape takes the place of this object's.
    Replace(u64),
    /// It is a new object.
    Add,
    /// It has not changed since it was last exchanged: this app's own is at least as new.
    Same,
    /// An instance of an object the package has already replaced: they share the one shape.
    Shared,
    /// It has no surface.
    Bare,
}

/// Decides each entry of a package, in order.
fn fates(scene: &Scene, link: &State, entries: &[LinkObject]) -> Vec<Fate> {
    let mut taken: Vec<u64> = Vec::new();
    let mut decide = |entry: &LinkObject| {
        if entry.indices.len() < 3 {
            return Fate::Bare;
        }
        let unchanged = !entry.same_as.is_empty();
        match target_of(scene, link, entry) {
            // An unchanged live copy is one the app draws itself.
            None if entry.copy && unchanged => Fate::Same,
            Some(_) if unchanged => Fate::Same,
            Some(object) if taken.contains(&object.mesh) => Fate::Shared,
            // An object squashed flat has no space of its own to carry points into.
            Some(object) if object.matrix().determinant().abs() > 1e-20 => {
                taken.push(object.mesh);
                Fate::Replace(object.id)
            }
            _ => Fate::Add,
        }
    };
    entries.iter().map(&mut decide).collect()
}

/// How Blender stands after a send.
enum Up {
    /// Open, with the link in it.
    Open,
    /// Started by this run a moment ago, and not up yet.
    Opening,
    /// Started now. `beside_another`: a Blender without the link was open
    /// already, which is not the one the objects go to.
    Started { beside_another: bool },
    /// A scripted run starts none: the package is left in the tray.
    Left,
}

fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 { format!("1 {one}") } else { format!("{count} {many}") }
}

impl Running {
    /// The Send to Blender button.
    pub(super) fn send_to_blender(&mut self) {
        match self.blender_send() {
            Ok(text) => {
                self.status(text, true);
                self.toast("Sent to Blender");
            }
            Err(why) => self.status(why, false),
        }
    }

    /// Writes the selected objects into Blender's tray and sees that there is
    /// a Blender to take them. Returns the line that says what went, or why
    /// nothing did.
    fn blender_send(&mut self) -> Result<String, String> {
        let Some(folder) = self.blender.folder.clone() else {
            return Err("This run of GolemSculpt keeps no settings folder, so it has no link to Blender.".into());
        };
        // Where Blender is, before anything is written: with none to start, nothing goes.
        let open = blender_is_open(&folder);
        let opening = !open && self.blender.opening.is_some_and(|(pid, at)| at.elapsed() < OPENING && process_is_running(pid));
        let start = if open || opening || self.blender.scripted { None } else { Some(self.blender_program(&folder)?) };

        let colours = self.export_paint();
        let doc = &self.doc;
        let out = outgoing(&doc.scene, &doc.mesh, &mut self.blender, &colours);
        if out.objects.is_empty() {
            let whose = if out.bare > 1 { "no selected object has" } else { "this object has no" };
            return Err(format!("There is nothing to send to Blender: {whose} surface."));
        }
        let since = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let file = folder.join(TO_BLENDER).join(format!("{since:013}-{}.{LINK_ENDING}", std::process::id()));
        io::write_link_package(&file, "golemsculpt", !doc.opts.flat_shading, &out.objects)?;
        for (entry, (key, print)) in out.objects.iter().zip(&out.prints) {
            self.blender.known.insert(*key, Known { mark: entry.mark.clone(), print: *print });
        }

        let up = match start {
            Some(exe) => {
                let script = put_script(&folder)?;
                // Asked before this one is started and would answer for itself.
                let beside_another = any_blender_running();
                let started = blender_command(&exe, &script).spawn();
                let started = started.map_err(|e| format!("Blender could not be started ({}): {e}", exe.display()))?;
                self.blender.opening = Some((started.id(), Instant::now()));
                Up::Started { beside_another }
            }
            None if open => {
                self.blender.opening = None;
                Up::Open
            }
            None if opening => Up::Opening,
            None => Up::Left,
        };

        let mut what = named(&out.names);
        if out.copies > 0 {
            what += &format!(" with {}", plural(out.copies, "live copy", "live copies"));
        }
        let they = if out.names.len() + out.copies == 1 { "It" } else { "They" };
        let mut text = match up {
            Up::Open => format!("Sent {what} to Blender."),
            Up::Opening => format!("Sent {what} to Blender, which is still opening. {they} will be there when it is up."),
            Up::Started { beside_another: false } => {
                format!("Sent {what} to Blender, and started Blender. {they} will be there when it has opened.")
            }
            // Only ever the first time: once the link is in Blender, every Blender that is opened has it.
            Up::Started { beside_another: true } => format!(
                "Sent {what} to Blender. The Blender that was open has no link to GolemSculpt yet, so a second one was \
                 started with it: {} will be there when it has opened. Close the other one; from now on every Blender \
                 you open has the link.",
                they.to_lowercase()
            ),
            Up::Left => format!("Left {what} for Blender in {}.", folder.display()),
        };
        if out.bare > 0 {
            text += &format!(" {} no surface and {} left out.", plural(out.bare, "selected object has", "selected objects have"), if out.bare == 1 { "was" } else { "were" });
        }
        if let Some(note) = io::note_saved(out.mended) {
            text += &format!(" {note}");
        }
        if matches!(up, Up::Open) && out.objects.iter().any(|o| o.paint.is_some()) && blender_link_is_old(&folder) {
            text += " The Blender that is open has the link from before paint, so the paint stays here: close Blender \
                     and press Send to Blender again, and GolemSculpt opens it with the new link.";
        }
        Ok(text)
    }

    /// The Blender to start: the one that is found, else the one the user
    /// shows, which is remembered. Only a person's own run comes here.
    fn blender_program(&mut self, folder: &Path) -> Result<PathBuf, String> {
        if let Some(found) = find_blender(folder) {
            return Ok(found);
        }
        let picked = rfd::FileDialog::new()
            .set_title("Blender was not found. Where is it? Choose blender.exe")
            .add_filter("Blender", &[if cfg!(windows) { "exe" } else { "*" }])
            .pick_file();
        let Some(picked) = picked else {
            return Err("Nothing was sent: GolemSculpt has to be shown where Blender is, once. Press Send to Blender again to choose it.".into());
        };
        let exe = chosen_blender(&picked).filter(|exe| usable(exe)).ok_or_else(|| {
            format!("{} is not Blender, so nothing was sent. Press Send to Blender again and choose blender.exe.", picked.display())
        })?;
        if let Err(e) = write_json(&folder.join("blender.json"), &json!({ "exe": exe.to_string_lossy() })) {
            log::warn!("{e}");
        }
        Ok(exe)
    }

    /// Takes in every package Blender has left. Called when the watch says
    /// there is one; leaves them for the next look while a stroke or a drag
    /// is under way, which holds what its undo step goes back to.
    pub(super) fn blender_arrivals(&mut self) {
        if self.doc.sculptor.is_active() || self.transform_dragging() || self.input.drag != Drag::None {
            return;
        }
        // A mesh coming back may take the place of one a deformation holds: that is applied first.
        self.deform_commit();
        let Some(tray) = self.blender.folder.as_ref().map(|folder| folder.join(TO_SCULPT)) else { return };
        for path in packages(&tray) {
            if older_than(&path, STALE) {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            // Another copy of the app may be watching the same tray: whoever renames it has it.
            let mine = path.with_extension(format!("taking{}", std::process::id()));
            if std::fs::rename(&path, &mine).is_err() {
                continue;
            }
            let read = io::read_link_package(&mine);
            let _ = std::fs::remove_file(&mine);
            match read {
                Ok(package) => self.take_from_blender(package),
                Err(why) => self.status(why, false),
            }
        }
        sweep(&tray);
        self.redraw = true;
    }

    /// Puts a package's objects into the scene as one undo step, and says what came.
    fn take_from_blender(&mut self, package: LinkPackage) {
        let colours = self.export_paint();
        let scene = &self.doc.scene;
        let fates = fates(scene, &self.blender, &package.objects);
        // The meshes first: building one is the long part, and nothing has changed yet.
        let (mut replaced, mut added) = (Vec::new(), Vec::new());
        let (mut same, mut bare, mut repaired) = (0, 0, 0);
        for (entry, fate) in package.objects.into_iter().zip(fates) {
            let (name, mark, origin) = (entry.name.clone(), entry.mark.clone(), entry.origin);
            match fate {
                Fate::Same | Fate::Shared => same += 1,
                Fate::Bare => bare += 1,
                Fate::Replace(id) => {
                    let Some(object) = scene.get(id) else { continue };
                    let (mut mesh, mended) = entry.into_mesh(object.matrix().inverse());
                    if mesh.indices.is_empty() {
                        bare += 1;
                        continue;
                    }
                    repaired += mended;
                    // Held against the object's own paint once that is on the new shape.
                    let theirs = paint::take_colour(&mut mesh);
                    replaced.push((id, object.name.clone(), mesh, mark, theirs));
                }
                Fate::Add => {
                    let (mesh, mended) = entry.into_mesh(Mat4::from_translation(-origin));
                    if mesh.indices.is_empty() {
                        bare += 1;
                        continue;
                    }
                    repaired += mended;
                    added.push((name, origin, mesh, mark));
                }
            }
        }

        let mut names = (Vec::new(), Vec::new());
        // Each object that arrived, with the mark it came under.
        let mut arrived: Vec<(u64, String)> = Vec::new();
        // What became of each replaced object's colour from Blender, for the status line.
        let mut painted: Vec<String> = Vec::new();
        if !replaced.is_empty() || !added.is_empty() {
            let mut new: Vec<u64> = Vec::new();
            self.edit_scene(|scene, slot| {
                for (id, name, mesh, mark, theirs) in replaced {
                    // The same model back from Blender keeps its face groups and its paint layers.
                    let mut mesh = super::groups::taking_over(scene, slot, id, mesh);
                    painted.extend(paint::said(&name, paint::came_back(&mut mesh, theirs, &colours)));
                    scene.replace_mesh_of(id, mesh, slot);
                    names.0.push(name);
                    arrived.push((id, mark));
                }
                for (name, origin, mesh, mark) in added {
                    let id = scene.add(&name, mesh, slot);
                    scene.active_mut().position = origin;
                    names.1.push(scene.active().name.clone());
                    arrived.push((id, mark));
                    new.push(id);
                }
                // What is new is what is selected, all of it; with nothing new the selection stays.
                if let Some((&first, rest)) = new.split_first() {
                    scene.select(first, slot);
                    for &id in rest {
                        scene.select_also(id);
                    }
                }
                true
            });
            self.doc.mesh_replaced = true;
            self.sync_view();
            if !new.is_empty() {
                self.frame_view();
            }
        }
        // What each is now is what Blender has: it goes back only if it is sculpted again.
        let doc = &self.doc;
        for (id, mark) in &arrived {
            let Some(object) = doc.scene.get(*id) else { continue };
            let Some(mesh) = doc.scene.mesh(object.mesh, &doc.mesh) else { continue };
            let print = print_of(mesh, object.matrix());
            self.blender.known.insert((*id, 0), Known { mark: mark.clone(), print });
        }
        for (id, _) in &arrived {
            self.flash.start(*id, flash::Kind::Select);
        }

        let mut parts = Vec::new();
        if !names.0.is_empty() {
            parts.push(format!("replaced {}", named(&names.0)));
        }
        if !names.1.is_empty() {
            parts.push(format!("added {}", named(&names.1)));
        }
        let changed = !parts.is_empty();
        let mut text = match (changed, same) {
            (true, _) => format!("From Blender: {}.", parts.join(" and ")),
            (false, 0) => "From Blender: nothing with a surface, so nothing was added.".to_owned(),
            (false, _) => "From Blender: nothing new. What was sent has not changed since it was last here.".to_owned(),
        };
        if changed && same > 0 {
            text += &format!(" {} unchanged, and left as {}.", same, if same == 1 { "it is" } else { "they are" });
        }
        if changed && bare > 0 {
            text += &format!(" {} no surface and {} left out.", plural(bare, "had", "had"), if bare == 1 { "was" } else { "were" });
        }
        for line in &painted {
            text += &format!(" {line}");
        }
        if package.paint_lost {
            text += " The paint Blender sent could not be read, so the shapes came without it.";
        }
        if changed {
            text += " Undo (Ctrl+Z) puts the scene back as it was.";
        }
        if let Some(note) = io::notes_opened(repaired, 0) {
            text += &format!(" {note}");
        }
        self.status(text, changed || same > 0);
        if changed {
            self.toast("From Blender");
        }
    }

    // ---- for visual checks ---------------------------------------------------

    /// `--demo-blender play`: the link worked through the routines the button
    /// and the watch run, with this run standing in for Blender (it writes the
    /// packages Blender's side would). Prints the status line each step leaves
    /// and a line per check (ok / FAIL). Nothing here starts Blender or goes
    /// near the user's own folder: the trays are in a scratch folder
    /// (`prefs::scratch`).
    ///
    /// `--demo-blender round` is the same road with a real Blender at the
    /// other end: it sends the scene's objects, waits for an answer in the
    /// tray (a Blender without a window, running `tests/blender_link/reply.py`
    /// on the same folder, gives one), takes it in and prints what it found.
    ///
    /// `--demo-blender live` is `round` with nothing held back: Blender is
    /// looked for where it is installed and started as the button starts it,
    /// window and all, so whoever runs this has to see to it that the Blender
    /// that opens answers and closes (`tests/blender_link/answer_and_quit.py`,
    /// in a scripts folder of the run's own) and works on the run's own
    /// folders (`tests/blender_link/README.md` has the commands).
    pub(super) fn blender_demo(&mut self, what: &str) {
        let folder = prefs::scratch("sculpt_blender_demo");
        if what == "play" {
            let _ = std::fs::remove_dir_all(&folder);
        }
        for tray in [TO_BLENDER, TO_SCULPT] {
            if let Err(e) = std::fs::create_dir_all(folder.join(tray)) {
                return log::error!("{e}");
            }
        }
        self.blender.folder = Some(folder.clone());
        self.blender.scripted = true;
        match what {
            "round" => self.blender_round(&folder),
            // No Blender to be found would mean a file box, which a scripted run never opens.
            "live" if find_blender(&folder).is_none() => println!("FAIL Blender was not found on this computer"),
            "live" => {
                self.blender.scripted = false;
                self.blender_round(&folder);
            }
            _ => self.blender_play(&folder),
        }
    }

    fn blender_round(&mut self, folder: &Path) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let (to_blender, to_sculpt) = (folder.join(TO_BLENDER), folder.join(TO_SCULPT));
        for path in packages(&to_blender).into_iter().chain(packages(&to_sculpt)) {
            let _ = std::fs::remove_file(path);
        }
        // Every object of the scene, as Ctrl + click on each row would select them.
        let ids: Vec<u64> = self.doc.scene.objects.iter().map(|o| o.id).collect();
        for &id in &ids {
            self.doc.scene.select_also(id);
        }
        let size = |app: &Running, id: u64| {
            let scene = &app.doc.scene;
            let object = scene.get(id)?;
            Some(scene.bounds_of(object, &app.doc.mesh).size())
        };
        let before: Vec<Option<Vec3>> = ids.iter().map(|&id| size(self, id)).collect();
        let count = self.doc.scene.objects.len();
        let triangles: usize = self.doc.scene.objects.iter().filter_map(|o| self.doc.scene.mesh(o.mesh, &self.doc.mesh)).map(Mesh::triangle_count).sum();
        let began = Instant::now();
        let sent = self.blender_send();
        println!("{}", sent.clone().unwrap_or_else(|why| why));
        println!("     {} triangles sent in {:.2} s", crate::printable::grouped(triangles), began.elapsed().as_secs_f64());
        // (A Blender that is already up may have taken the package by now.)
        say("the scene's objects were sent", sent.is_ok() && (self.blender.opening.is_some() || packages(&to_blender).len() == 1));

        // Blender's turn: `reply.py` takes the package and sends its answer.
        let began = Instant::now();
        while packages(&to_sculpt).is_empty() && began.elapsed() < Duration::from_secs(180) {
            std::thread::sleep(Duration::from_millis(200));
        }
        say("Blender answered", !packages(&to_sculpt).is_empty());
        let began = Instant::now();
        self.blender_arrivals();
        println!("{}", self.ui.status.0);
        println!("     taken in in {:.2} s", began.elapsed().as_secs_f64());
        let after: Vec<Option<Vec3>> = ids.iter().map(|&id| size(self, id)).collect();
        for ((id, was), now) in ids.iter().zip(&before).zip(&after) {
            let name = self.doc.scene.get(*id).map(|o| o.name.clone()).unwrap_or_default();
            if let (Some(was), Some(now)) = (was, now) {
                println!("     \"{name}\": {:.4} x {:.4} x {:.4} before, {:.4} x {:.4} x {:.4} after", was.x, was.y, was.z, now.x, now.y, now.z);
            }
        }
        println!("     {} objects before, {} after", count, self.doc.scene.objects.len());
        say("the tray is empty again", packages(&to_sculpt).is_empty());
    }

    fn blender_play(&mut self, folder: &Path) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let (to_blender, to_sculpt) = (folder.join(TO_BLENDER), folder.join(TO_SCULPT));
        let newest = |tray: &Path| packages(tray).pop().and_then(|path| io::read_link_package(&path).ok());
        // Blender's answer: a package in this app's tray, then the watch's look at it.
        let answer = |app: &mut Running, objects: &[LinkObject]| {
            let file = to_sculpt.join(format!("{:013}-1.{LINK_ENDING}", app.blender.marks + 1_000));
            let wrote = io::write_link_package(&file, "blender", true, objects);
            app.blender_arrivals();
            println!("{}", app.ui.status.0);
            wrote.is_ok()
        };
        // The middle and the size of a set of points, and of an object as it stands in the scene.
        let reach = |points: &mut dyn Iterator<Item = Vec3>| {
            let (low, high) = points.fold((Vec3::MAX, Vec3::MIN), |(low, high), p| (low.min(p), high.max(p)));
            ((low + high) * 0.5, high - low)
        };
        let extent = |app: &Running, id: u64| {
            let scene = &app.doc.scene;
            let object = scene.get(id)?;
            let (mesh, place) = (scene.mesh(object.mesh, &app.doc.mesh)?, object.matrix());
            Some(reach(&mut mesh.positions.iter().map(|p| place.transform_point3(*p))))
        };
        let near = |a: Vec3, b: Vec3| (a - b).length() < 1e-3 * (1.0 + b.length());

        // The object the app starts with, moved, turned and resized: what goes is where it stands.
        let ball = self.doc.scene.active_id();
        {
            let object = self.doc.scene.active_mut();
            object.position = Vec3::new(3.0, 1.0, -2.0);
            object.rotation = glam::Quat::from_rotation_y(0.6);
            object.scale = Vec3::new(2.0, 1.0, 0.5);
        }
        self.sync_view();
        let (centre, size) = extent(self, ball).unwrap_or_default();
        let name = self.doc.scene.active().name.clone();
        let sent = self.blender_send();
        println!("{}", sent.clone().unwrap_or_else(|why| why));
        let first = newest(&to_blender);
        let went = first.as_ref().map(|package| package.objects.as_slice()).unwrap_or_default();
        say("Send to Blender leaves one package in Blender's tray", sent.is_ok() && packages(&to_blender).len() == 1);
        say("a scripted run starts no Blender", self.blender.opening.is_none() && sent.is_ok_and(|text| text.starts_with("Left")));
        say("the object is in it under its name, as itself", went.len() == 1 && went[0].name == name && !went[0].copy && went[0].same_as.is_empty());
        let (their_centre, their_size) = went.first().map(|o| reach(&mut o.positions.iter().copied())).unwrap_or_default();
        say("where it stands in the scene, at the size it is shown", near(their_centre, centre) && near(their_size, size));
        say("with its centre", went.first().is_some_and(|o| near(o.origin, Vec3::new(3.0, 1.0, -2.0))));

        // Sent again untouched: it says it is the same.
        let again = self.blender_send().is_ok();
        let second = newest(&to_blender);
        let same = second.as_ref().and_then(|package| package.objects.first()).map(|o| (o.mark.clone(), o.same_as.clone()));
        say("sent again unchanged, it says so", again && same.is_some_and(|(mark, same_as)| mark == same_as && !mark.is_empty() && Some(&mark) == went.first().map(|o| &o.mark)));

        // Blender sends it back as it got it: nothing happens here.
        let steps = self.doc.history.steps().0;
        let uid = self.doc.mesh.uid;
        let mut back = went.first().cloned().unwrap_or_default();
        back.sent_as = back.name.clone();
        back.same_as = back.mark.clone();
        let wrote = answer(self, std::slice::from_ref(&back));
        say("what comes back unchanged is left alone", wrote && self.doc.history.steps().0 == steps && self.doc.mesh.uid == uid);
        say("and the tray is emptied", packages(&to_sculpt).is_empty());

        // Blender worked on it: twice as tall where it stands, moved 5 along X; and made a new object.
        let mut worked = back.clone();
        (worked.mark, worked.same_as) = ("b1".into(), String::new());
        for p in &mut worked.positions {
            *p = Vec3::new(p.x + 5.0, centre.y + (p.y - centre.y) * 2.0, p.z);
        }
        let mut made = LinkObject::placed(&crate::mesh::primitives::build(crate::mesh::primitives::Primitive::Cube, 500, 1.5), Mat4::from_translation(Vec3::new(0.0, 4.0, 9.0))).0;
        (made.name, made.mark) = ("Mold Half".into(), "b2".into());
        let placed = {
            let object = self.doc.scene.active();
            (object.position, object.rotation, object.scale)
        };
        let count = self.doc.scene.objects.len();
        let wrote = answer(self, &[worked.clone(), made.clone()]);
        let now = extent(self, ball).unwrap_or_default();
        say("an object Blender worked on takes its new shape, in one undo step", wrote && self.doc.history.steps().0 == steps + 1);
        say("where Blender had it: 5 along X, twice as tall", near(now.0, centre + Vec3::X * 5.0) && near(now.1, size * Vec3::new(1.0, 2.0, 1.0)));
        let kept = self.doc.scene.get(ball).map(|o| (o.position, o.rotation, o.scale));
        say("the object keeps its own centre, turn and size", kept == Some(placed));
        let mold = self.doc.scene.objects.iter().find(|o| o.name == "Mold Half").map(|o| (o.id, o.position));
        say("what was made in Blender is a new object", self.doc.scene.objects.len() == count + 1 && mold.is_some());
        let mold_id = mold.map_or(0, |(id, _)| id);
        say("centred where Blender had its centre", mold.is_some_and(|(_, at)| near(at, Vec3::new(0.0, 4.0, 9.0))));
        say("and 3 across, as it was there", extent(self, mold_id).is_some_and(|(at, size)| near(at, Vec3::new(0.0, 4.0, 9.0)) && near(size, Vec3::splat(3.0))));
        say("it is the one selected", self.doc.scene.active_id() == mold_id && self.doc.scene.selected() == [mold_id]);

        // Both go back to Blender untouched: both say they are the same.
        self.doc.scene.select_also(ball);
        let sent = self.blender_send().is_ok();
        let third = newest(&to_blender);
        let marks: Vec<(String, String, String)> = third.iter().flat_map(|p| &p.objects).map(|o| (o.name.clone(), o.mark.clone(), o.same_as.clone())).collect();
        let said_same = marks.len() == 2 && marks.iter().all(|(_, mark, same_as)| mark == same_as) && marks.iter().any(|(_, mark, _)| mark == "b1") && marks.iter().any(|(_, mark, _)| mark == "b2");
        say("sent back untouched, each goes under the mark it came with", sent && said_same);

        // Undo puts the old shape back and takes the new object away; then the ball is not what Blender has.
        self.act(crate::doc::Action::Undo);
        let undone = extent(self, ball).unwrap_or_default();
        say("Undo puts the scene back as it was", self.doc.scene.objects.len() == count && near(undone.0, centre) && near(undone.1, size));
        self.act(crate::doc::Action::Redo);
        say("and Redo brings what came from Blender back", self.doc.scene.objects.len() == count + 1 && extent(self, ball).is_some_and(|(_, now)| near(now, size * Vec3::new(1.0, 2.0, 1.0))));

        // The new object comes again, changed: it is replaced, not added twice. Blender says which
        // it is by the mark it last went under, so it is found whatever it is called here by now.
        if let Some(object) = self.doc.scene.get_mut(mold_id) {
            object.name = "Lower Mold".into();
        }
        let mut remade = made.clone();
        (remade.sent_as, remade.was, remade.mark) = ("Mold Half".into(), "b2".into(), "b3".into());
        remade.positions.iter_mut().for_each(|p| p.y += 1.0);
        let wrote = answer(self, std::slice::from_ref(&remade));
        say("sent a second time, it replaces itself", wrote && self.doc.scene.objects.len() == count + 1 && extent(self, mold_id).is_some_and(|(at, _)| near(at, Vec3::new(0.0, 5.0, 9.0))));

        // A copy that Blender changed has no object here to be: it comes as a new one.
        let mut copy = worked.clone();
        (copy.name, copy.copy, copy.mark) = ("Sphere_MirrorX".into(), true, "b4".into());
        let wrote = answer(self, std::slice::from_ref(&copy));
        say("a live copy that was worked on arrives as an object of its own", wrote && self.doc.scene.objects.len() == count + 2);

        // A file that is not a package says so and changes nothing.
        let steps = self.doc.history.steps().0;
        let wrote = std::fs::write(to_sculpt.join(format!("9999999999999-1.{LINK_ENDING}")), b"GSLINK01 and then nothing").is_ok();
        self.blender_arrivals();
        println!("{}", self.ui.status.0);
        say("a damaged package is refused, and cleared away", wrote && self.doc.history.steps().0 == steps && packages(&to_sculpt).is_empty());

        // Where Blender is looked for.
        let libraries = steam_libraries("\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}\n");
        say("Steam's libraries are read from its list", libraries == [PathBuf::from(r"C:\Program Files (x86)\Steam"), PathBuf::from(r"D:\SteamLibrary")]);
        say("the script Blender runs is written beside the trays", put_script(folder).is_ok_and(|path| std::fs::read(path).is_ok_and(|there| there == SCRIPT.as_bytes())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::primitives::{self, Primitive};

    fn link() -> State {
        State { folder: None, run: "run".into(), marks: 0, known: HashMap::new(), opening: None, scripted: true }
    }

    fn entry(name: &str, reference: &str) -> LinkObject {
        let mesh = primitives::build(Primitive::Cube, 100, 0.5);
        let mut entry = LinkObject::placed(&mesh, Mat4::IDENTITY).0;
        (entry.name, entry.reference, entry.mark) = (name.into(), reference.into(), "b1".into());
        entry
    }

    #[test]
    fn what_goes_is_every_selected_object_with_its_copies_each_under_a_mark() {
        let mut slot = primitives::build(Primitive::Sphere, 1_000, 0.5);
        let mut scene = Scene::new("Ball");
        let ball = scene.active_id();
        let block = scene.add("Tall Block", primitives::build(Primitive::Cube, 500, 0.5), &mut slot);
        let empty = scene.add("Empty", Mesh::new(Vec::new(), Vec::new(), None), &mut slot);
        scene.get_mut(block).unwrap().position = Vec3::new(3.0, 0.0, 0.0);
        scene.set_mirror(ball, 1, Vec3::ZERO);
        scene.select(block, &mut slot);
        scene.select_also(empty);
        scene.select_also(ball);

        let mut link = link();
        let out = outgoing(&scene, &slot, &mut link, &io::ExportPaint::default());
        let said: Vec<(&str, bool, &str)> = out.objects.iter().map(|o| (o.name.as_str(), o.copy, o.reference.as_str())).collect();
        let (ball_ref, block_ref) = (format!("run:{ball}"), format!("run:{block}"));
        assert_eq!(said, [("Ball", false, ball_ref.as_str()), ("Ball_MirrorX", true, ball_ref.as_str()), ("Tall Block", false, block_ref.as_str())]);
        assert_eq!((out.names.as_slice(), out.copies, out.bare), (&["Ball".to_owned(), "Tall Block".to_owned()][..], 1, 1));
        assert!(out.objects.iter().all(|o| !o.mark.is_empty() && o.same_as.is_empty()), "nothing has been exchanged yet");
        assert_eq!(out.objects[2].origin, Vec3::new(3.0, 0.0, 0.0));
        let mut marks: Vec<&str> = out.objects.iter().map(|o| o.mark.as_str()).collect();
        marks.dedup();
        assert_eq!(marks.len(), 3, "each sending has its own");

        // Once that has gone, the same again says it is the same; a moved object does not.
        for (entry, (key, print)) in out.objects.iter().zip(&out.prints) {
            link.known.insert(*key, Known { mark: entry.mark.clone(), print: *print });
        }
        scene.get_mut(block).unwrap().position = Vec3::new(4.0, 0.0, 0.0);
        let again = outgoing(&scene, &slot, &mut link, &io::ExportPaint::default());
        let same: Vec<bool> = again.objects.iter().zip(&out.objects).map(|(now, was)| now.same_as == was.mark && now.mark == was.mark).collect();
        assert_eq!(same, [true, true, false]);
        assert!(again.objects[2].same_as.is_empty() && again.objects[2].mark != out.objects[2].mark);
    }

    #[test]
    fn each_thing_blender_sends_finds_its_place() {
        let mut slot = primitives::build(Primitive::Sphere, 1_000, 0.5);
        let mut scene = Scene::new("Ball");
        let ball = scene.active_id();
        let twin = scene.instance_active();
        let block = scene.add("Block", primitives::build(Primitive::Cube, 500, 0.5), &mut slot);
        let flat = scene.add("Flat", primitives::build(Primitive::Cube, 500, 0.5), &mut slot);
        scene.get_mut(flat).unwrap().scale = Vec3::new(1.0, 0.0, 1.0);
        let named = |name: &str, sent_as: &str| LinkObject { sent_as: sent_as.into(), ..entry(name, "") };
        let same = |mut entry: LinkObject| {
            entry.same_as = entry.mark.clone();
            entry
        };
        let entries = [
            // This run's own number wins, whatever the object is called by now.
            entry("Renamed in Blender", &format!("run:{ball}")),
            // Its instance shares the shape that has just been replaced.
            entry("Twin", &format!("run:{twin}")),
            // Another run's number means nothing here: the name it went under does.
            named("Block.001", "Block"),
            // Made in Blender: nothing to be.
            entry("Mold Half", ""),
            // A number of this run's that names no object any more.
            entry("Gone", "run:999"),
            // Unchanged since it was last here.
            same(named("Block", "Block")),
            // A live copy: unchanged, the app's own to draw; changed, a new object.
            same(LinkObject { copy: true, ..entry("Ball_MirrorX", &format!("run:{ball}")) }),
            LinkObject { copy: true, ..entry("Ball_MirrorX", &format!("run:{ball}")) },
            // No surface.
            LinkObject { indices: Vec::new(), ..entry("Nothing", "") },
            // An object squashed flat cannot take points into its own space.
            named("Flat", "Flat"),
        ];
        // The block came from Blender earlier in this run, as "Cube", and is called "Block" here.
        let mut link = link();
        link.known.insert((block, 0), Known { mark: "b7".into(), print: 0 });
        let entries = entries.into_iter().chain([LinkObject { sent_as: "Cube".into(), was: "b7".into(), ..entry("Cube", "") }]).collect::<Vec<_>>();
        let said: Vec<String> = fates(&scene, &link, &entries)
            .into_iter()
            .map(|fate| match fate {
                Fate::Replace(id) => format!("replace {}", scene.get(id).unwrap().name),
                Fate::Add => "add".into(),
                Fate::Same => "same".into(),
                Fate::Shared => "shared".into(),
                Fate::Bare => "bare".into(),
            })
            .collect();
        assert_eq!(said, ["replace Ball", "shared", "replace Block", "add", "add", "same", "same", "add", "bare", "add", "shared"]);
        assert_eq!(block, scene.objects[2].id);
    }

    #[test]
    fn blender_is_looked_for_where_it_is_installed() {
        assert!(version_of("Blender 4.10") > version_of("Blender 4.2"));
        assert!(version_of("Blender 5.2") > version_of("Blender 4.10"));
        let list = "\"libraryfolders\"\n{\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t\t\"label\"\t\t\"\"\n\t}\n}\n";
        assert_eq!(steam_libraries(list), [PathBuf::from(r"D:\SteamLibrary")]);
        assert!(steam_libraries("not a list").is_empty());

        let program = if cfg!(windows) { "blender.exe" } else { "blender" };
        let folder = Path::new("some").join("where");
        assert_eq!(chosen_blender(&folder.join(program)), Some(folder.join(program)));
        let launcher = if cfg!(windows) { "blender-launcher.exe" } else { "blender-launcher" };
        assert_eq!(chosen_blender(&folder.join(launcher)), Some(folder.join(program)));
        assert_eq!(chosen_blender(&folder.join("sculpt.exe")), None);
        // Nothing on a network share is so much as asked about.
        assert!(!usable(Path::new(r"\\server\share\blender.exe")));

        // The note a Blender that has gone left behind is cleared away; a live one's counts.
        let folder = std::env::temp_dir().join("sculpt_blender_link_tests");
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        assert!(!blender_is_open(&folder));
        let gone = folder.join("blender-4294967290.json");
        std::fs::write(&gone, b"{}").unwrap();
        assert!(!blender_is_open(&folder) && !gone.exists());
        std::fs::write(folder.join(format!("blender-{}.json", std::process::id())), b"{}").unwrap();
        assert!(blender_is_open(&folder));

        // The watch says nothing while the tray is empty, and speaks up when a package lands in it.
        let tray = folder.join("watched");
        std::fs::create_dir_all(&tray).unwrap();
        let (arrived, woken) = (Arc::new(AtomicBool::new(false)), Arc::new(std::sync::atomic::AtomicUsize::new(0)));
        let (flag, count, watched) = (arrived.clone(), woken.clone(), tray.clone());
        // The loop "goes" at the second call, which ends the watch.
        let watch = std::thread::spawn(move || watch_tray(&watched, &flag, || count.fetch_add(1, Ordering::AcqRel) < 1));
        std::thread::sleep(LOOK * 2);
        assert!(!arrived.load(Ordering::Acquire) && woken.load(Ordering::Acquire) == 0);
        std::fs::write(tray.join("1.gslink.tmp"), b"x").unwrap();
        std::thread::sleep(LOOK * 2);
        assert!(!arrived.load(Ordering::Acquire), "a package still being written is not one yet");
        std::fs::rename(tray.join("1.gslink.tmp"), tray.join("1.gslink")).unwrap();
        watch.join().unwrap();
        assert!(arrived.load(Ordering::Acquire) && woken.load(Ordering::Acquire) == 2, "said at each look while it waits");

        // A package is one whole file with the link's ending; one being written is not seen.
        let tray = folder.join(TO_SCULPT);
        std::fs::create_dir_all(&tray).unwrap();
        for name in ["2.gslink", "1.gslink", "3.gslink.tmp", "4.taking77"] {
            std::fs::write(tray.join(name), b"x").unwrap();
        }
        assert_eq!(packages(&tray), [tray.join("1.gslink"), tray.join("2.gslink")]);
    }

    #[test]
    fn a_blender_with_the_link_from_before_paint_is_told_apart() {
        let folder = std::env::temp_dir().join("sculpt_blender_link_version_tests");
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        assert!(!blender_link_is_old(&folder));
        let mine = folder.join(format!("blender-{}.json", std::process::id()));
        std::fs::write(&mine, br#"{"pid": 1, "link": 2}"#).unwrap();
        assert!(!blender_link_is_old(&folder));
        // A note from before paint said link 1, or nothing at all.
        std::fs::write(&mine, br#"{"pid": 1, "link": 1}"#).unwrap();
        assert!(blender_link_is_old(&folder));
        std::fs::write(&mine, br#"{"pid": 1}"#).unwrap();
        assert!(blender_link_is_old(&folder));
        // A Blender that has gone does not count.
        std::fs::remove_file(&mine).unwrap();
        std::fs::write(folder.join("blender-4294967290.json"), br#"{"link": 1}"#).unwrap();
        assert!(!blender_link_is_old(&folder));
    }

    #[test]
    fn painted_objects_go_with_their_paint_and_a_repaint_is_a_change() {
        let mut slot = primitives::build(Primitive::Sphere, 1_000, 0.5);
        let red = vec![crate::mesh::paint::Rgba8([200, 0, 0, 255]); slot.vertex_count()];
        slot.set_paint(Some(crate::io::painted::imported_stack(red)));
        let mut scene = Scene::new("Ball");
        let ball = scene.active_id();
        scene.select(ball, &mut slot);
        let mut link = link();
        let paint = io::ExportPaint::default();
        let out = outgoing(&scene, &slot, &mut link, &paint);
        let sent = out.objects[0].paint.as_ref().expect("its paint goes with it");
        assert_eq!(sent.composite.len(), slot.vertex_count());
        for (entry, (key, print)) in out.objects.iter().zip(&out.prints) {
            link.known.insert(*key, Known { mark: entry.mark.clone(), print: *print });
        }
        // Painted again here, it is not what Blender has any more.
        let mut stack = slot.paint_stack().unwrap().into_owned();
        stack.layers[0].data.colour.as_mut().unwrap()[0] = crate::mesh::paint::Rgba8([1, 2, 3, 255]);
        slot.set_paint(Some(stack));
        let again = outgoing(&scene, &slot, &mut link, &paint);
        assert!(again.objects[0].same_as.is_empty(), "a repaint is a change");
    }
}
