//! **Nothing opens the person's print by its descriptor** (the security review of `build/print-protected`, S1, 2026-11-14). While the person's print of
//! a document they opened with its password is prepared, and while it's with the system's printing, the app holds a descriptor on a plain PDF of those
//! pages, with no name. `/dev/fd/N`, or a link to it, opened that descriptor again: an agent at Control got a plain tab of it (not protected, no
//! password, no copy flag), or its pages added to a tab of its own, and every printed page from `render`. The file wall
//! (`recto_platform::document_file`) refuses a path that leads into `/dev`, before anything is looked at, and a file with no link, after it's opened,
//! for every session and actor; this is what it must hold in the app.

use super::print_protected::deadlines_met;
use super::printing::{ALL, answer, hold_at, printed, unnamed_file};
use super::withheld_events::{person_unlocks, protected};
use crate::testkit::headless::{Disk, HeadlessApp, WAIT_LIMIT};
use recto_agent::{AgentError, Code, Level, exit};
use std::path::{Path, PathBuf};

/// **The descriptors this process holds on regular files with no link**, by number: where the person's print is while it's anywhere.
fn unnamed_descriptors() -> Vec<u32> {
    use std::os::unix::fs::MetadataExt;
    let mut held: Vec<u32> = std::fs::read_dir("/dev/fd")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let number = entry.file_name().to_str()?.parse().ok()?;
            let meta = std::fs::metadata(entry.path()).ok()?;
            (meta.is_file() && meta.nlink() == 0).then_some(number)
        })
        .collect();
    held.sort_unstable();
    held
}

/// What an answer was, in a line: a refusal's code and exit, or what the agent got, with page 1 of the tab it names drawn by an agent at Full.
fn said(app: &HeadlessApp, got: Result<serde_json::Value, AgentError>, out: &Path) -> String {
    match got {
        Err(error) => format!("{:?} exit {}", error.code, error.exit),
        Ok(answer) => {
            let tab = answer["tab"].as_u64().or_else(|| answer["active"]["tab"].as_u64()).unwrap_or(0);
            let tab = format!("#{tab}");
            let _ = std::fs::remove_file(out);
            let drawn = app.recto(Level::Full, &["render", "--page", "1", "--tab", &tab, "--out", path_str(out)]);
            let bytes = std::fs::metadata(out).map_or(0, |meta| meta.len());
            format!(
                "OPENED {answer} | render of page 1: {} ({bytes} bytes of PNG)",
                drawn.map_or_else(|error| error.message, |_| "drawn".into())
            )
        }
    }
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("a path in words")
}

/// **Every way an agent at `level` names descriptor `n` as a file to read**, each answer in a line: `open` and `page insert` (into `own`, a plain tab),
/// by `/dev/fd/N` and by a link to it in a temp folder.
fn tries(app: &HeadlessApp, level: Level, n: u32, own: u64, out: &Path) -> Vec<String> {
    let fd = format!("/dev/fd/{n}");
    let link = app.disk().files().join(format!("link-{n}-{level:?}.pdf"));
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(&fd, &link).expect("a link in the temp folder");
    let own = format!("#{own}");
    let mut lines = Vec::new();
    for (what, path) in [("", fd.as_str()), (" by a link", path_str(&link))] {
        let opened = app.recto(level, &["open", path]);
        lines.push(format!("{level:?} open{what}: {}", said(app, opened, out)));
        let inserted = app.recto(level, &["page", "insert", path, "--at", "1", "--tab", &own]);
        lines.push(format!("{level:?} page insert{what}: {}", said(app, inserted, out)));
    }
    lines
}

/// **What a child started by the app's own spawn (`spawn_detached`, as the updater's are) holds with no link**, as it lists its own descriptors: each
/// `/dev/fd/N` that is a regular file with a link count of 0, then "end". The person's print's file is made close-on-exec (std's `OpenOptions` always
/// is), so nothing; a render worker gets only its channel whatever the app holds (`isolate`'s `a_child_inherits_only_its_channel`).
fn a_childs_unnamed_files(app: &HeadlessApp) -> String {
    let log = app.disk().files().join("child.log");
    // One `stat` of every descriptor the shell has, which the `stat` inherits as they are: its link count, its kind and its name, a line each.
    let script = "stat -L -f '%l %HT %N' /dev/fd/* 2>/dev/null; echo end";
    recto_platform::local::spawn_detached(Path::new("/bin/sh"), &["-c".into(), script.into()], &[], &log).expect("a child started");
    app.wait_until("the child has listed its descriptors", || {
        std::fs::read_to_string(&log).is_ok_and(|said| said.ends_with("end\n"))
    });
    let listed = std::fs::read_to_string(&log).expect("the child's log");
    assert!(
        listed.lines().any(|line| line.starts_with("1 Regular File")),
        "the child's own log, named, is listed: {listed}"
    );
    listed
        .lines()
        .filter(|line| *line == "end" || line.starts_with("0 Regular File"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// What each try must say: by its level at Read-only, and the wall's one refusal, "This file couldn't be read as a PDF.", at Control and Full.
fn refused(level: Level) -> Vec<String> {
    let line = match level {
        Level::Control | Level::Full => format!("{:?} exit {}", Code::NOT_PDF, exit::FAILED),
        _ => format!("{:?} exit {}", Code::PERMISSION_DENIED, exit::PERMISSION),
    };
    ["open", "page insert", "open by a link", "page insert by a link"]
        .iter()
        .map(|what| format!("{level:?} {what}: {line}"))
        .collect()
}

/// **Failure (the security review's S1): an agent opens the person's print of a protected document by its descriptor**, `/dev/fd/N` or a link to it, as
/// a plain tab or as pages added to a tab of its own, and draws every printed page: the person's own session isn't confined, an unlinked file behind
/// `/dev/fd/N` is a regular file, and the open dups the descriptor. While the print is prepared (held after its first page) and once it's ready for the
/// system's printing, every descriptor the app holds on a file with no link is tried at Read-only, Control and Full: each is refused, by its level or by
/// the wall, nothing opens, and the person's print still prints. And (F3) a child the app starts while the print is held holds no file with no link.
#[test]
fn an_agent_cannot_open_the_persons_print_by_its_descriptor() {
    deadlines_met();
    let disk = Disk::new().as_the_users_own();
    let plain = disk.pdf("plain.pdf", 1);
    let file = protected(&disk, 2, "Locked");
    let app = disk.start();
    app.open(&plain);
    let own = app.tabs().iter().map(|row| u64::from(row.tab)).max().expect("the plain tab");
    person_unlocks(&app, &file);
    let tabs = app.tabs().len();
    let out = app.disk().files().join("drawn.png");
    let before = unnamed_descriptors();
    let made = || -> Vec<u32> { unnamed_descriptors().into_iter().filter(|n| !before.contains(n)).collect() };
    let try_all = |when: &str| {
        let held = made();
        assert!(!held.is_empty(), "{when}: the print's file is held with no link");
        for n in held {
            for level in [Level::ReadOnly, Level::Control, Level::Full] {
                assert_eq!(tries(&app, level, n, own, &out), refused(level), "{when}, descriptor {n}");
            }
        }
        assert_eq!(app.tabs().len(), tabs, "{when}: no tab opened");
    };

    let (held, go) = hold_at(1);
    answer(&app, ALL);
    held.recv_timeout(WAIT_LIMIT).expect("the print held after its first page");
    try_all("while it's prepared");
    assert_eq!(a_childs_unnamed_files(&app), "end", "a child the app starts while the print is held (F3)");
    drop(go);
    app.settle();
    try_all("once it's ready");
    assert_eq!(printed(&app), "[612,612]", "the person's print still prints");
    let edits = app.recto(Level::Full, &["edits", "--tab", &format!("#{own}")]).expect("the plain tab's edits");
    assert_eq!(
        (&edits["edited"], &edits["pages"]),
        (&false.into(), &1.into()),
        "no page added to the agent's tab: {edits}"
    );
}

/// The refusal an agent at Full gets for opening `path`, whole, as JSON: what twins must be equal in.
fn refusal_of(app: &HeadlessApp, path: &str) -> String {
    match app.recto(Level::Full, &["open", path]) {
        Ok(answer) => format!("OPENED {answer}"),
        Err(error) => format!("{} {} {:?} {:?} {:?}", error.code, error.exit, error.message, error.next, error.untrusted),
    }
}

/// **Failure (S1's twins): the refusal of `/dev/fd/N` says whether N is open, and so when a print is held**: before the wall a descriptor not open was
/// "no such file", one on a named file opened it, and one on a file with no link opened that. Every path under `/dev` gets the one refusal before
/// anything is looked at, the same whatever is behind it, and so does a link to one in a temp folder, a link to a descriptor that isn't open included;
/// and the person who tries it is told the same by the page.
#[test]
fn every_path_into_dev_gets_one_refusal_whatever_is_behind_it() {
    use std::os::fd::AsRawFd;
    let disk = Disk::new().as_the_users_own();
    let named = disk.pdf("named.pdf", 1);
    let app = disk.start();
    let unnamed = unnamed_file(&std::fs::read(&named).expect("a PDF's bytes"));
    let open_elsewhere = std::fs::File::open(&named).expect("a named file held open");
    let paths: Vec<String> = [
        format!("/dev/fd/{}", unnamed.as_raw_fd()),
        format!("/dev/fd/{}", open_elsewhere.as_raw_fd()),
        "/dev/fd/999".into(),
        "/dev/stdin".into(),
        "/dev/null".into(),
        "/dev".into(),
        format!("//dev/./fd/{}", unnamed.as_raw_fd()),
    ]
    .into();
    let links: Vec<PathBuf> = paths
        .iter()
        .enumerate()
        .map(|(n, target)| {
            let link = app.disk().files().join(format!("twin-{n}.pdf"));
            std::os::unix::fs::symlink(target, &link).expect("a link");
            link
        })
        .collect();
    let mut answers: Vec<(String, String)> = paths.iter().map(|path| (path.clone(), refusal_of(&app, path))).collect();
    answers.extend(links.iter().map(|link| (link.display().to_string(), refusal_of(&app, path_str(link)))));
    let first = answers[0].1.clone();
    assert!(first.starts_with("not_pdf "), "the wall's refusal: {first}");
    for (path, answer) in &answers {
        assert_eq!(answer, &first, "{path} is told otherwise than {}", answers[0].0);
    }
    assert!(app.tabs().is_empty(), "nothing opened");
    app.open(Path::new(&paths[0]));
    assert_eq!(
        app.core().window.notices().open_failure().as_deref(),
        Some(recto_core::OpenError::NotPdf.message()),
        "the person is told by the page as for any file that isn't a PDF"
    );
    assert!(app.tabs().is_empty(), "nothing opened for the person");
    drop((unnamed, open_elsewhere));
}

/// **Failure (the wall refusing what a person opens): a file the person can open today is refused by the wall**, as one reached through a link, one with
/// a second name, one another program holds open, or one on another volume than the temp folder (this checkout's `target`, where it is another: on CI
/// it may be the same). Each opens for the person and for an agent at Control, as before, in a session that isn't confined to the temp folders.
#[test]
fn the_wall_leaves_every_file_a_person_opens() {
    let disk = Disk::new().as_the_users_own();
    let plain = disk.pdf("plain.pdf", 1);
    let linked = disk.files().join("linked.pdf");
    std::os::unix::fs::symlink(disk.pdf("by-a-link.pdf", 1), &linked).expect("a link");
    let second = disk.files().join("second-name.pdf");
    std::fs::hard_link(disk.pdf("hard.pdf", 1), &second).expect("a second name");
    let held = disk.pdf("held.pdf", 1);
    let _open_elsewhere = std::fs::File::open(&held).expect("held open by another");
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    std::fs::create_dir_all(&target).expect("the checkout's target");
    let other = recto_platform::fresh::FreshDir::new_in(&target, "recto-wall-volume-", "").expect("a folder in target");
    let elsewhere = other.path().join("elsewhere.pdf");
    std::fs::copy(&plain, &elsewhere).expect("a copy on the other volume");
    let app = disk.start();
    for (n, path) in [&plain, &linked, &second, &held, &elsewhere].into_iter().enumerate() {
        if n % 2 == 0 {
            app.open(path);
            assert_eq!(app.core().window.notices().open_failure(), None, "the person opens {}", path.display());
        } else {
            app.recto(Level::Control, &["open", path_str(path)])
                .unwrap_or_else(|error| panic!("an agent opens {}: {error:?}", path.display()));
            app.settle();
        }
        assert_eq!(app.tabs().len(), n + 1, "{} opened", path.display());
    }
}
