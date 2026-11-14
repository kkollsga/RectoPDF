//! **Every open of a file by its path in the crates a command's path reaches goes through the file wall** ([`super::document_file`]), or is on a
//! reviewed list with its reason. A new `File::open` of a path an agent names would open `/dev/fd/N`, the person's print, again (S1); this fails it until
//! it goes through the wall or is listed, by a reviewer, as never given such a path.

use std::path::{Path, PathBuf};

/// The crates whose code a path from a command, a record, a restore, a drop or an Apple event reaches.
const CRATES: &[&str] = &[
    "recto-agent",
    "recto-app",
    "recto-control",
    "recto-core",
    "recto-ffi",
    "recto-index",
    "recto-reader",
    "recto-render-service",
    "recto-session",
];

/// What opens a file by its path, other than the wall.
const OPENS: &[&str] = &[
    "File::open(",
    "File::options(",
    "OpenOptions::new(",
    "FileSource::open(",
    "fs::read(",
    "fs::read_to_string(",
];

/// **The reviewed list:** each file of those crates that opens a file by its path other than through the wall, how many times, and why none of those
/// paths can be one a caller names as a document. Sorted by file.
const OPENS_OUTSIDE_THE_WALL: &[(&str, usize, &str)] = &[
    (
        "recto-agent/src/policy_guards.rs",
        5,
        "a test module (`#[cfg(test)] mod policy_guards` in lib.rs): the test's own files",
    ),
    (
        "recto-agent/src/render.rs",
        4,
        "files this process makes and names itself: an export's temp file (`OwnTemp::create`, `create_new`), the print's file with no name \
         (`create_unnamed`, `create_new`), an output written new (`create_new`, never over a file), and the temp file it made read back to copy it there",
    ),
    (
        "recto-agent/src/skill.rs",
        3,
        "`recto skill install`, the user's own command in `recto`'s process (Route::Here, UserOnly): its own SKILL.md in the folder named, read to compare \
         and written new; never a document, never in the app",
    ),
    (
        "recto-control/src/record_store.rs",
        2,
        "the state folder's own record files, named by the app: flushed to the drive, and a kept copy written",
    ),
    ("recto-control/src/record_store/lock.rs", 1, "the state folder's lock file, named by the app"),
    ("recto-control/src/server.rs", 1, "the socket's lock file in the state folder, named by the app"),
    (
        "recto-control/src/terminal.rs",
        1,
        "`/dev/tty`, by that name only: the terminal `recto` asks the person at, in `recto`'s own process",
    ),
    ("recto-core/src/save.rs", 1, "a save's own temp file, flushed before it's renamed into place"),
    (
        "recto-index/src/corpus.rs",
        3,
        "the index's own files (its table, its pack, its lock) under the app's index folder",
    ),
];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|ending| ending == "rs") {
            out.push(path);
        }
    }
}

/// Whether a file of a crate's sources is a test's alone by where it is: a `tests` folder, a `*tests.rs`, the headless kit's tests or the testkit.
fn a_tests_file(name: &str) -> bool {
    name.contains("/tests/") || name.ends_with("tests.rs") || name.contains("/headless/") || name.contains("/testkit/")
}

/// The opens in a file's code: before its first `#[cfg(test)]`, and not in a line comment.
fn opens_in(text: &str) -> usize {
    let code = text.split("#[cfg(test)]").next().unwrap_or_default();
    code.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .map(|line| OPENS.iter().map(|open| line.matches(open).count()).sum::<usize>())
        .sum()
}

/// **Failure (S1 again, by a new route): a new open of a file by its path in these crates, outside the wall,** that a path an agent names could reach:
/// `/dev/fd/N` then opens the person's print, or any descriptor of the app's, again. Every such open is on the reviewed list with its reason, at its
/// count, and nothing else opens a path; the count also falls when one goes, so the list is never wider than the code.
#[test]
fn every_open_of_a_path_goes_through_the_wall_or_is_reviewed() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut found = Vec::new();
    for krate in CRATES {
        let mut paths = Vec::new();
        sources(&crates.join(krate).join("src"), &mut paths);
        for path in paths {
            let name = path.strip_prefix(crates).unwrap().to_string_lossy().replace('\\', "/");
            let opens = opens_in(&std::fs::read_to_string(&path).unwrap());
            if opens > 0 && !a_tests_file(&name) {
                found.push((name, opens));
            }
        }
    }
    found.sort();
    let listed: Vec<(String, usize)> = OPENS_OUTSIDE_THE_WALL.iter().map(|(name, count, _)| ((*name).to_owned(), *count)).collect();
    assert_eq!(
        found, listed,
        "a file is opened by its path outside the wall (recto_platform::document_file::open) where OPENS_OUTSIDE_THE_WALL doesn't say why: open a \
         document through the wall, or list a file of the app's own with its reason"
    );
}

/// **Failure: the scan can't see an open**, so the list above passes whatever the code does. A seeded open of each kind is counted, in code; and not in a
/// comment, or in a file's tests.
#[test]
fn the_wall_scan_sees_a_seeded_open() {
    for open in OPENS {
        let seeded = format!("fn read(path: &Path) {{\n    let _ = std::{open}path);\n}}\n");
        assert_eq!(opens_in(&seeded), 1, "{open} in: {seeded}");
        let commented: String = seeded.lines().map(|line| format!("// {line}\n")).collect();
        assert_eq!(opens_in(&commented), 0, "{open} in a comment");
        assert_eq!(
            opens_in(&format!("fn f() {{}}\n#[cfg(test)]\nmod tests {{\n{seeded}}}\n")),
            0,
            "{open} in tests"
        );
    }
    assert!(a_tests_file("recto-app/src/core/headless/printing.rs") && !a_tests_file("recto-agent/src/render.rs"));
}
