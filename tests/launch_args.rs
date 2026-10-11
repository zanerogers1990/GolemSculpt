//! Starts a program the way the Claude bridge's `launch` starts the app, and
//! checks what the program was given.
//!
//! `launch` hands the app the file to open as an argument, and nothing in
//! the file's name may be read as anything but the name: not a space, a
//! quote, an `&`, a `%VAR%`, a `^`, nor a backslash at the end of it. The
//! unit tests in `src/mcp.rs` show that no shell is involved. This shows the
//! rest: that the name arrives whole, character for character, after Windows
//! has carried it across twice as part of a command line (from the server to
//! the go-between, and from the go-between to the app), which is where a
//! quote or a closing backslash goes wrong if it is written out carelessly.
//!
//! It runs the real program, `sculpt.exe --start-app <app> <file>`, which is
//! the go-between `launch` uses, with this test program standing in for the
//! app: started with `SCULPT_TEST_ARGS_TO` set, it writes down the arguments
//! it was given and the folder it was started in, and exits. No window is
//! opened and no port is touched.
//!
//! It is not built on the test harness (`harness = false` in `Cargo.toml`):
//! the stand-in has to take any argument at all, and the harness would read
//! a file's name as the name of a test. So it is one check, run by a plain
//! `cargo test`; with a test's name after it (`cargo test weld`) it runs
//! only if that name is part of `launch_args`.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Set for the stand-in: the file it is to write what it was given to.
const NOTE: &str = "SCULPT_TEST_ARGS_TO";
const NAME: &str = "launch_args";
/// How long the stand-in has to start and write its note.
const WAIT: Duration = Duration::from_secs(30);

/// What `launch` can be asked to open. Most are not files that could exist
/// (Windows allows no `"` in a name), but all of them can be asked for, and
/// the go-between hands on whatever it is given.
const NAMES: &[&str] = &[
    r"C:\Models\head.sclp",
    r"C:\Models\two words.sclp",
    r"C:\Models\this & that.obj",
    r"C:\Models\50% %PATH% %USERPROFILE% %TEMP%.stl",
    r"C:\Models\caret ^ and (brackets) 'quote'.sclp",
    r"C:\Models\pipes | < > ; , = ! ` ~ @ # $ { } [ ] +.obj",
    r"C:\Models\-leading-dash.sclp",
    r"C:\Models\--demo",
    "--demo",
    "--mcp",
    "-x",
    // A backslash at the end, with and without a space to make quoting necessary.
    r"C:\Models\folder\",
    r"C:\Models\folder\\",
    r"C:\Models\a folder\",
    r"C:\Models\a folder\\",
    // Quotes, and backslashes before a quote.
    "C:\\Models\\said \"quoted\" & echo gotcha > x.txt",
    "C:\\Models\\a backslash\\\"before a quote.sclp",
    "C:\\Models\\two backslashes\\\\\"before a quote.sclp",
    "C:\\Models\\ends with a backslash and a quote\\\"",
    "C:\\Models\\ends with a quote and a backslash\"\\",
    "a\" & calc & rem \"",
    "\"",
    "\"\"",
    "\\",
    "\\\\",
    "\\\"",
    " ",
    "",
    "a\ttab",
    "two\nlines",
    "C:\\Models\\mod\u{e8}le \u{6a21}\u{578b} \u{1f5ff}.sclp",
];

fn main() {
    if let Some(note) = std::env::var_os(NOTE) {
        write_down(Path::new(&note));
        return;
    }
    if !asked_for() {
        println!("{NAME}: not asked for");
        return;
    }
    let sculpt = Path::new(env!("CARGO_BIN_EXE_sculpt"));
    let me = std::env::current_exe().expect("this program has a path");
    let scratch = me.with_file_name(format!("launch-args-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    // The stand-in twice over: where it is, and in a folder whose own name wants care.
    let odd = scratch.join("odd dir & 100% ^ (x)").join("stand in.exe");
    std::fs::create_dir_all(odd.parent().unwrap()).expect("a folder beside the test program");
    std::fs::copy(&me, &odd).expect("a copy of the test program");

    let mut failures = 0;
    let mut check = |app: &Path, file: Option<&str>| {
        let outcome = arrives_whole(sculpt, app, file, &scratch);
        if let Err(why) = &outcome {
            failures += 1;
            println!("FAIL {file:?}: {why}");
        }
    };
    for name in NAMES {
        check(&me, Some(name));
    }
    for name in [r"C:\Models\two words.sclp", "said \"quoted\" & echo gotcha", r"C:\Models\a folder\"] {
        check(&odd, Some(name));
    }
    // No file at all is no argument at all.
    check(&me, None);
    check(&odd, None);
    let _ = std::fs::remove_dir_all(&scratch);
    let checked = NAMES.len() + 5;
    if failures > 0 {
        println!("{NAME}: {failures} of {checked} names did not arrive as they were sent");
        std::process::exit(1);
    }
    println!("{NAME}: ok. {checked} names arrived whole, each as one argument");
}

/// Whether to run: `cargo test` passes on whatever was typed after it.
fn asked_for() -> bool {
    let typed: Vec<String> = std::env::args().skip(1).collect();
    let names: Vec<&String> = typed.iter().filter(|word| !word.starts_with('-')).collect();
    let not_for_this = typed.iter().any(|word| word == "--ignored" || word == "--list");
    !not_for_this && (names.is_empty() || names.iter().any(|name| NAME.contains(name.as_str())))
}

/// The stand-in's whole job: what it was given, and where it was started.
fn write_down(note: &Path) {
    let mut lines: Vec<String> = std::env::args_os().skip(1).map(|arg| format!("arg {}", spelt(&arg))).collect();
    let folder = std::env::current_dir().map(PathBuf::into_os_string).unwrap_or_default();
    lines.push(format!("folder {}", spelt(&folder)));
    // Written beside the note and moved into place, so the note is never read half written.
    let part = note.with_extension("part");
    std::fs::write(&part, lines.join("\n")).and_then(|()| std::fs::rename(&part, note)).expect("the note is written");
}

/// A string as its code units, in hex: exact whatever is in it.
fn spelt(text: &OsStr) -> String {
    #[cfg(windows)]
    let units: Vec<u16> = std::os::windows::ffi::OsStrExt::encode_wide(text).collect();
    #[cfg(not(windows))]
    let units: Vec<u16> = text.to_string_lossy().encode_utf16().collect();
    units.iter().map(|unit| format!("{unit:04x}")).collect::<Vec<String>>().join(" ")
}

/// Runs the go-between as the server does (`mcp::errand_command`: the option,
/// the app, the file, each an argument of its own) and checks what the
/// stand-in was started with by it.
fn arrives_whole(sculpt: &Path, app: &Path, file: Option<&str>, scratch: &Path) -> Result<(), String> {
    let note = scratch.join("note.txt");
    let _ = std::fs::remove_file(&note);
    let mut errand = Command::new(sculpt);
    errand.arg("--start-app").arg(app).args(file).env(NOTE, &note).stdin(Stdio::null());
    let done = errand.output().map_err(|e| format!("could not run {}: {e}", sculpt.display()))?;
    let said = String::from_utf8_lossy(&done.stdout);
    if !done.status.success() || said.trim().parse::<u32>().is_err() {
        let errors = String::from_utf8_lossy(&done.stderr);
        return Err(format!("the go-between gave no process number: {said:?} {errors:?}"));
    }
    let began = Instant::now();
    while !note.is_file() {
        if began.elapsed() > WAIT {
            return Err("the program that was started wrote nothing down".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let got = std::fs::read_to_string(&note).map_err(|e| e.to_string())?;
    let folder = app.parent().map(Path::as_os_str).unwrap_or_default();
    let mut expected: Vec<String> = file.iter().map(|name| format!("arg {}", spelt(OsStr::new(name)))).collect();
    expected.push(format!("folder {}", spelt(folder)));
    let got: Vec<String> = got.lines().map(|line| folded(line, "folder ")).collect();
    let expected: Vec<String> = expected.iter().map(|line| folded(line, "folder ")).collect();
    if got == expected {
        return Ok(());
    }
    // Said in letters, for whoever reads it; it was compared in code units.
    let readable = |lines: &[String]| -> Vec<String> {
        let shown = |line: &String| match line.strip_prefix("arg") {
            Some(units) => format!("arg {:?}", decoded(units)),
            None => line.clone(),
        };
        lines.iter().map(shown).collect()
    };
    Err(format!("sent {:?}, arrived as {:?}", readable(&expected), readable(&got)))
}

/// A line of the note, with the folder's line made safe to compare: Windows
/// hands a folder back with its own slashes, and may give the drive letter in
/// the other case. The arguments are compared exactly as they are.
fn folded(line: &str, folder: &str) -> String {
    let Some(units) = line.strip_prefix(folder) else { return line.to_owned() };
    let text = decoded(units).to_string_lossy().to_lowercase().replace('/', "\\");
    format!("{folder}{}", text.trim_end_matches('\\'))
}

/// The other way from [`spelt`].
fn decoded(units: &str) -> OsString {
    let units: Vec<u16> = units.split_whitespace().filter_map(|unit| u16::from_str_radix(unit, 16).ok()).collect();
    String::from_utf16_lossy(&units).into()
}
