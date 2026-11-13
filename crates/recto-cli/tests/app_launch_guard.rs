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
    assert!(found.is_empty(), "these start the build folder's own app; run a copy (`OwnCopy`) instead:\n{}", found.join("\n"));
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
