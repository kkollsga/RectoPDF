//! **A test never starts the build folder's own app by hand.** Starting the built app registers it with the system, and a test that runs its executable itself ends with nothing that forgets the build folder afterwards: the registration stays until some other test's end forgets it, and the registrations stage fails on a second RectoPDF in Open With when none does (CI run 43369527265: `app/build.noindex/RectoPDF.app`, "not a bed's folder; on disk"). Shown by hand: 20 of 20 direct runs of the built binary that ended at once left the build folder registered.
//!
//! Reads the test sources, starts nothing: it is not an app test and is not counted by the ratchet.

use std::path::{Path, PathBuf};

/// The test sources, every file under `tests/`.
fn sources(folder: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(folder).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

/// What a `Command::new(...)` on this line is given, if it is the build folder's executable: the built path itself, or a variable named for it.
fn starts_the_build(line: &str) -> Option<String> {
    let code = line.split("//").next().unwrap_or("");
    let mut rest = code;
    while let Some(at) = rest.find("Command::new(") {
        let after = &rest[at + "Command::new(".len()..];
        let argument = after.split(')').next().unwrap_or("").trim().trim_start_matches('&').trim();
        let direct = matches!(argument, "app" | "built") || argument.contains("app_path") || argument.contains("build.noindex");
        if direct {
            return Some(argument.to_owned());
        }
        rest = after;
    }
    None
}

/// Failure: a test runs the built app's executable itself (`Command::new(&app)`), which registers the build folder with Launch Services and leaves it there for the registrations stage to find. A test that needs the real binary runs a copy (`OwnCopy`), which is forgotten, with a look, when it is dropped.
#[test]
fn no_test_starts_the_build_folders_app_itself() {
    let mut files = Vec::new();
    sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests"), &mut files);
    assert!(files.len() > 20, "found no test sources");
    let mut found = Vec::new();
    for file in files {
        if file.file_name().is_some_and(|name| name == "app_launch_guard.rs") {
            continue;
        }
        for (at, line) in std::fs::read_to_string(&file).unwrap().lines().enumerate() {
            if let Some(argument) = starts_the_build(line) {
                found.push(format!("{}:{}: Command::new({argument})", file.display(), at + 1));
            }
        }
    }
    assert!(
        found.is_empty(),
        "these start the build folder's own app; run a copy (`OwnCopy`) instead:\n{}",
        found.join("\n")
    );
}

/// Failure: the check above stops seeing the shapes it is for, and passes on code that starts the app.
#[test]
fn the_check_sees_the_shapes_it_is_for() {
    for line in [
        "let out = Command::new(&app).arg(\"--qa\").output().unwrap();",
        "Command::new(app).spawn()",
        "Command::new(app_path().unwrap())",
        "Command::new(\"../../app/build.noindex/RectoPDF.app/Contents/MacOS/RectoPDF\")",
        "let a = Command::new(bin()).status(); Command::new(&built)",
    ] {
        assert!(starts_the_build(line).is_some(), "{line}");
    }
    for line in [
        "Command::new(copy.executable()).arg(\"--qa\")",
        "Command::new(bin()).args([\"qa\", \"start\", \"--app\", app.to_str().unwrap()])",
        "// Command::new(&app) is what the guard is about",
        "Command::new(\"/bin/kill\")",
    ] {
        assert!(starts_the_build(line).is_none(), "{line}");
    }
}

/// Failure: quiet-app cleanup signals an unowned numeric PID after exit, risking a signal to an unrelated process that reused it.
#[test]
fn quiet_run_cleanup_never_signals_a_borrowed_pid() {
    let source = include_str!("common/app.rs");
    for (name, code) in [
        (
            "Run::kill",
            source.split("pub fn kill(mut self)").nth(1).expect("Run::kill").split("\n}").next().unwrap(),
        ),
        (
            "Run::drop",
            source.split("impl Drop for Run<'_>").nth(1).expect("Run::drop").split("\n}").next().unwrap(),
        ),
    ] {
        for forbidden in ["/bin/kill", "signal_process(", "libc::kill("] {
            assert!(!code.contains(forbidden), "{name} signals a borrowed PID through {forbidden}");
        }
    }
}

#[allow(dead_code)]
#[path = "common/app/lifetime.rs"]
mod lifetime;

#[cfg(unix)]
fn waiting_child() -> std::process::Child {
    use std::io::BufRead;
    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", "printf 'ready\n'; exec /bin/cat"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    assert_eq!(line, "ready\n");
    child
}

/// Failure: hard cleanup is attempted on an attached process, or an owned killed child is left unreaped and its PID unanchored.
#[cfg(unix)]
#[test]
fn owned_quiet_children_are_reaped_and_borrowed_instances_cannot_be_killed() {
    eprintln!(
        "test harness compiler sizes: Child={}, Lifetime={}",
        std::mem::size_of::<std::process::Child>(),
        std::mem::size_of::<lifetime::Lifetime>()
    );
    let mut borrowed = lifetime::Lifetime::Borrowed(None);
    assert!(borrowed.kill_owned(std::time::Duration::ZERO).unwrap_err().contains("requires"));
    let child = waiting_child();
    let pid = child.id();
    let mut owned = lifetime::Lifetime::Owned(child);
    owned.kill_owned(std::time::Duration::from_secs(1)).unwrap();
    assert!(owned.gone(pid).unwrap(), "the Child is reaped, not just absent from a PID lookup");
    owned.kill_owned(std::time::Duration::ZERO).unwrap();
}

