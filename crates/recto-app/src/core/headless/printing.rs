//! **The person's File > Print, from the print panel's answer to the file handed to the system's printing** (S2's guard, put in before the person's print
//! became the `print` command run as theirs): which pages each answer prints, the notices said and their buttons, Stop mid-way, a second print asked while
//! one is being prepared, and a file that asks not to be copied, on paper and to a file, with "Print anyway".

use super::files::locked_allowing;
use crate::core::controls::UiAction;
use crate::core::printing::{AFTER_END, AT_PAGE};
use crate::lock;
use crate::testkit::headless::{Disk, HeadlessApp, WAIT_LIMIT, print_patience};
use std::path::Path;

/// What the print panel answered: all the pages or a range from 1, the selection's pages, and whether it prints to a file (a PDF saved or previewed).
#[derive(Clone, Copy)]
pub(super) struct Panel {
    pub(super) all: bool,
    pub(super) first: u32,
    pub(super) last: u32,
    pub(super) selection_only: bool,
    pub(super) to_file: bool,
}

pub(super) const ALL: Panel = Panel {
    all: true,
    first: 1,
    last: 1,
    selection_only: false,
    to_file: false,
};

/// **The panel answered, as the host answers it:** the tab showing and what was chosen, through the one way a person's action comes in.
pub(super) fn answer(app: &HeadlessApp, panel: Panel) {
    let tab = lock(&app.core().published).reader.shown_tab;
    let Panel {
        all,
        first,
        last,
        selection_only,
        to_file,
    } = panel;
    app.act(UiAction::Print {
        tab,
        all,
        first,
        last,
        selection_only,
        to_file,
        serial: 1,
    });
}

/// The notice by the page as the window shows it now: its words and its button's title.
pub(super) fn notice(app: &HeadlessApp) -> String {
    match app.reader().selection_notice {
        None => "-".into(),
        Some(notice) => format!("{:?} [{}]", notice.text, notice.action_title.unwrap_or_default()),
    }
}

/// **The file the print made, taken as the host takes it and let go as the system's printing does:** the width in points of each page in it, which says
/// which pages of the document it holds, as each page of the test files has its own width. Its file has no name and is the user's alone while it's there,
/// and it goes with its descriptor when it's let go. "none" if there's no file for the host.
pub(super) fn printed(app: &HeadlessApp) -> String {
    let Some((path, _)) = app.core().tools.printing().take_ready() else {
        return "none".into();
    };
    let bytes = std::fs::read(&path).expect("the print's file, by the path the host is given");
    let text = String::from_utf8_lossy(&bytes);
    let widths: Vec<&str> = text
        .match_indices("/MediaBox [0 0 ")
        .filter_map(|(at, found)| text[at + found.len()..].split(' ').next())
        .collect();
    let (links, mode, owner, identity) = {
        use std::os::unix::fs::MetadataExt;
        let file = std::fs::metadata(&path).expect("the file");
        (file.nlink(), file.mode() & 0o777, file.uid(), (file.dev(), file.ino()))
    };
    assert_eq!(
        (links, mode, owner),
        (0, 0o600, own_uid()),
        "{} has no name and is the user's alone",
        path.display()
    );
    app.core().tools.printing().finished(path.to_str().expect("a path"));
    // Only the print's own file is looked for, by its identity, so what other threads or tests open or close meanwhile can't change the answer: the
    // number it had is gone or another file's now, and no descriptor of ours is still on it.
    if let Ok(now) = std::fs::metadata(&path) {
        use std::os::unix::fs::MetadataExt;
        assert_ne!(
            (now.dev(), now.ino()),
            identity,
            "the print's file goes with its descriptor once the system's printing is done with it: {} still is it",
            path.display()
        );
    }
    let still: Vec<Unnamed> = unnamed_descriptors_in(Path::new("/dev/fd"))
        .into_iter()
        .filter(|held| (held.dev, held.ino) == identity)
        .collect();
    assert!(
        still.is_empty(),
        "the print's file goes with its descriptor once the system's printing is done with it: still held as {still:?}"
    );
    format!("[{}]", widths.join(","))
}

/// This process's user.
fn own_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    let probe = recto_testpdf::TempDir::new("uid-");
    std::fs::metadata(probe.path()).expect("a folder of ours").uid()
}

/// The print files of this process's in the system's temp folder, made or half made, named as prints were before they had no name, or as the unnamed one
/// is for the moment before it's unlinked: what no print may leave, or have while it's prepared.
pub(super) fn print_files_left() -> Vec<String> {
    let folder = recto_platform::local::system_temp_dir().unwrap_or_else(std::env::temp_dir);
    let pid = std::process::id();
    std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| is_print_file_of(name, pid))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a name in the temp folder is one of process `pid`'s print files.
fn is_print_file_of(name: &str, pid: u32) -> bool {
    name.starts_with(&format!("recto-print-{pid}-")) || name.starts_with(&format!(".recto-{pid}-")) || name.starts_with(&format!(".recto-unnamed-{pid}-"))
}

/// **The files with no name this process holds a descriptor of,** by inode, with each one's permissions, sorted: where the person's print's file is while
/// it's anywhere, as nothing else can reach it. Counted before and after a step, the difference is what the step made or let go.
pub(crate) fn unnamed_files_held() -> Vec<(u64, u32)> {
    unnamed_files_held_in(Path::new("/dev/fd"))
}

/// [`unnamed_files_held`] of the descriptors listed in `folder` (`/dev/fd`, or another to test the look itself).
pub(crate) fn unnamed_files_held_in(folder: &Path) -> Vec<(u64, u32)> {
    let mut held: Vec<(u64, u32)> = unnamed_descriptors_in(folder).into_iter().map(|held| (held.ino, held.mode)).collect();
    held.sort_unstable();
    held.dedup();
    held
}

/// One descriptor of this process on a regular file with no link.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Unnamed {
    pub(crate) fd: u32,
    pub(crate) dev: u64,
    pub(crate) ino: u64,
    pub(crate) mode: u32,
}

/// **The descriptors listed in `folder` that are regular files with no link, strictly:** a folder that can't be listed, or an entry that can't be read,
/// panics with the OS error, as a look that quietly saw nothing read as "nothing's held" (2026-11-25, `[] then []`). A descriptor that closed between
/// the listing and the look (ENOENT, EBADF) is skipped, as it's legitimately gone; any other error on it panics with its errno. Sorted by number.
/// The skip of a descriptor that closed is not exercised by a test: it needs a close in the instant between the listing and the look.
pub(crate) fn unnamed_descriptors_in(folder: &Path) -> Vec<Unnamed> {
    use std::os::unix::fs::MetadataExt;
    let mut held = Vec::new();
    for entry in std::fs::read_dir(folder).unwrap_or_else(|error| panic!("the descriptors in {} can't be listed: {error}", folder.display())) {
        let entry = entry.unwrap_or_else(|error| panic!("an entry of {} can't be read: {error}", folder.display()));
        let Some(fd) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else {
            continue;
        };
        let meta = match std::fs::metadata(entry.path()) {
            Ok(meta) => meta,
            // Closed between the listing and the look.
            Err(error) if matches!(error.raw_os_error(), Some(ENOENT | EBADF)) => continue,
            Err(error) => panic!("descriptor {fd} in {} can't be looked at: {error}", folder.display()),
        };
        if meta.is_file() && meta.nlink() == 0 {
            held.push(Unnamed {
                fd,
                dev: meta.dev(),
                ino: meta.ino(),
                mode: meta.mode() & 0o777,
            });
        }
    }
    held.sort_unstable_by_key(|held| held.fd);
    held
}

/// errno values on this platform (macOS and Linux agree), so no `libc` dependency is needed for two numbers.
const ENOENT: i32 = 2;
const EBADF: i32 = 9;

/// A file with no name holding `bytes`, as a print's is, for a test of what holds it.
pub(crate) fn unnamed_file(bytes: &[u8]) -> std::fs::File {
    use std::io::Write;
    let folder = recto_testpdf::TempDir::new("unnamed-");
    let path = folder.join("print");
    let mut file = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path).expect("a file");
    std::fs::remove_file(&path).expect("unlinked");
    file.write_all(bytes).expect("written");
    file
}

/// A print held once it's this many pages in, till the test lets it go: the sender that lets it go.
pub(super) fn hold_at(page: u32) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
    let (told, held) = std::sync::mpsc::channel();
    let (go, waiting) = std::sync::mpsc::channel();
    *lock(&AT_PAGE) = Some((page, (told, waiting)));
    (held, go)
}

/// A file of `count` pages, each its own width: 200 points, then 210, and so on.
fn widths(count: u32) -> Vec<u8> {
    let list: Vec<recto_testpdf::Page> = (0..count)
        .map(|n| recto_testpdf::Page {
            media: (200.0 + 10.0 * n as f32, 792.0),
            ..recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("Page {}", n + 1)]))
        })
        .collect();
    recto_testpdf::pages(&list)
}

/// What a person's print did, a line each, on 2026-11-14 before it became the `print` command run as theirs.
const GOLDEN: &[&str] = &[
    "all | printed [200,210,220,230] | after -",
    "pages 2 to 3 | printed [210,220] | after -",
    "the selection, all of it, over a range of page 1 | printed [200,210,220,230] | after -",
    "all, to a file | printed [200,210,220,230] | after -",
    "stopped after a page | while \"Preparing pages for printing: 2 of 4.\" [Stop] | printed none | after - | left []",
    "a second asked while one's prepared | while \"Preparing pages for printing: 2 of 4.\" [Stop] | printed [200,210,220,230] | after \"A print is already being prepared.\" [] | printed none",
    "asks not to be copied, on paper | printed [612,612] | after -",
    "asks not to be copied, to a file | printed none | after \"This file's permissions ask that it isn't copied.\" [Print anyway]",
    "its Print anyway | printed [612,612] | after -",
];

/// Failure (S2, a guard put in before the person's print moved into the executor): the person's print does other than it did: prints other pages than the
/// panel chose (all, a range, the selection's), says other words or offers another button, or none, at any step (preparing, stopped, refused, a second asked),
/// asks "anyway" on paper or doesn't to a file of a document that asks not to be copied, prints nothing on "Print anyway", leaves a file behind when it's
/// stopped, or gives the host a file it didn't when stopped or refused. One app, each answer in turn, held against the golden.
#[test]
fn a_persons_print_does_what_it_did() {
    let disk = Disk::new();
    let doc = disk.file("widths.pdf", &widths(4));
    let restricted = disk.file("restricted.pdf", &locked_allowing("", 2, lopdf::encryption::Permissions::PRINTABLE));
    let app = disk.start();
    app.open(&doc);
    app.settle();
    let mut lines = Vec::new();
    let mut plain = |name: &str, panel: Panel| {
        answer(&app, panel);
        app.settle();
        let printed = printed(&app);
        app.settle();
        lines.push(format!("{name} | printed {printed} | after {}", notice(&app)));
    };
    plain("all", ALL);
    plain(
        "pages 2 to 3",
        Panel {
            all: false,
            first: 2,
            last: 3,
            ..ALL
        },
    );
    // Select All selects every page: the panel's range of page 1 is overridden by the selection's pages when it prints just those.
    app.core().perform("edit.select_all".into());
    app.settle();
    plain(
        "the selection, all of it, over a range of page 1",
        Panel {
            all: false,
            selection_only: true,
            ..ALL
        },
    );
    plain("all, to a file", Panel { to_file: true, ..ALL });

    // Stop, once a page is done.
    let (held, go) = hold_at(1);
    answer(&app, ALL);
    held.recv_timeout(print_patience(1)).expect("the print held at its first page");
    app.wait_until("the notice says it's preparing", || notice(&app).contains("2 of 4"));
    let preparing = notice(&app);
    let stop = app.reader().selection_notice.and_then(|notice| notice.action_id).expect("Stop");
    app.act(UiAction::Menu { id: stop });
    drop(go);
    app.settle();
    let stopped = printed(&app);
    app.settle();
    lines.push(format!(
        "stopped after a page | while {preparing} | printed {stopped} | after {} | left {:?}",
        notice(&app),
        print_files_left()
    ));

    // A second print asked while one's prepared.
    let (held, go) = hold_at(1);
    answer(&app, ALL);
    held.recv_timeout(print_patience(1)).expect("the print held at its first page");
    app.wait_until("the notice says it's preparing", || notice(&app).contains("2 of 4"));
    answer(&app, ALL);
    app.wait_until("the window looked again", || app.core().core.flush(WAIT_LIMIT));
    let preparing = notice(&app);
    drop(go);
    app.settle();
    let first = printed(&app);
    app.settle();
    let after = notice(&app);
    let second = printed(&app);
    lines.push(format!(
        "a second asked while one's prepared | while {preparing} | printed {first} | after {after} | printed {second}"
    ));
    app.core().dismiss_selection_notice();
    app.settle();

    app.open(&restricted);
    app.settle();
    let mut plain = |name: &str, panel: Panel| {
        answer(&app, panel);
        app.settle();
        let printed = printed(&app);
        app.settle();
        lines.push(format!("{name} | printed {printed} | after {}", notice(&app)));
    };
    plain("asks not to be copied, on paper", ALL);
    plain("asks not to be copied, to a file", Panel { to_file: true, ..ALL });
    let anyway = app.reader().selection_notice.and_then(|notice| notice.action_id).expect("Print anyway");
    app.act(UiAction::Menu { id: anyway });
    app.settle();
    let printed_anyway = printed(&app);
    app.settle();
    lines.push(format!("its Print anyway | printed {printed_anyway} | after {}", notice(&app)));

    assert_eq!(lines, GOLDEN, "\nnow:\n{}", lines.join("\n"));
    assert_eq!(print_files_left(), Vec::<String>::new(), "nothing's left once each print's file went");
}

/// **Failure (PLAN, "the person can't print a document they opened with its password", the design's point 3): the person's print has a name on disk,**
/// where an agent at any level could open it while it's there (0.1.0 named it `recto-print-PID-N.pdf`) and a crash would leave it, a plain copy of a
/// document opened with its password once Print reads one; or it's readable by others; or its descriptor outlives the print on some ending. While it's
/// prepared (held mid-way) and once it's ready: nothing of it in the temp folder by any name, and one file with no link held, 0600. After each ending (done,
/// Stop, the tab closed, quitting with it ready): none held. A crash or a kill is covered by the link count of 0 from the file's making: the kernel frees a
/// file with no link when its last descriptor closes, which a process's end does, however it ends.
#[test]
fn the_persons_print_has_no_name() {
    let disk = Disk::new();
    let doc = disk.file("widths.pdf", &widths(3));
    let app = disk.start();
    app.open(&doc);
    app.settle();
    let before = unnamed_files_held();
    let made = || -> Vec<(u64, u32)> { unnamed_files_held().into_iter().filter(|held| !before.contains(held)).collect() };
    let held_mid_way = |step: &str| {
        let (held, go) = hold_at(1);
        answer(&app, ALL);
        held.recv_timeout(WAIT_LIMIT).expect("the print held at its first page");
        assert_eq!(print_files_left(), Vec::<String>::new(), "{step}: nothing by a name while it's prepared");
        let now = made();
        assert!(now.len() == 1 && now[0].1 == 0o600, "{step}: one file with no name, the user's alone: {now:?}");
        go
    };

    drop(held_mid_way("done"));
    app.settle();
    assert!(app.core().tools.printing().has_ready(), "ready for the host");
    assert_eq!(print_files_left(), Vec::<String>::new(), "nothing by a name once it's ready");
    assert_eq!(made().len(), 1, "held while it's ready");
    assert_ne!(printed(&app), "none");
    assert_eq!(made(), Vec::new(), "done: let go");

    let go = held_mid_way("stopped");
    let stop = app.reader().selection_notice.and_then(|notice| notice.action_id).expect("Stop");
    app.act(UiAction::Menu { id: stop });
    drop(go);
    app.settle();
    assert_eq!(
        (printed(&app), made()),
        ("none".into(), Vec::new()),
        "stopped: nothing for the host, nothing held"
    );

    let go = held_mid_way("the tab closed");
    app.act(UiAction::Menu { id: "file.close".into() });
    app.wait_until("the tab closed", || app.tabs().is_empty());
    drop(go);
    app.settle();
    assert_eq!(
        (printed(&app), made()),
        ("none".into(), Vec::new()),
        "the tab closed: nothing for the host, nothing held"
    );

    app.open(&doc);
    app.settle();
    answer(&app, ALL);
    app.settle();
    assert_eq!(made().len(), 1, "ready");
    app.core().tools.printing().quitting();
    assert_eq!(made(), Vec::new(), "quitting with it ready: let go");
    assert_eq!(print_files_left(), Vec::<String>::new());
}

/// Failure (S2, the coordinator's word on 2026-11-14: "a person's action that fails says so"): a person's print refused before its command ran (by a
/// quiet run's files, as here, or a page list that can't be read) says nothing, as the window's watch over it never began; or it leaves the window
/// thinking a print's being prepared. The refusal's own words are said, and nothing is prepared or left for the host.
#[test]
fn a_print_refused_before_it_runs_says_its_words() {
    let disk = Disk::new();
    let doc = disk.pdf("a.pdf", 2);
    let app = disk.start();
    app.open(&doc);
    app.settle();
    let Some(outside) = crate::testkit::headless::outside_the_tests_folders("printed.pdf") else {
        return;
    };
    let line = format!(r#"{{"cmd":"print","to_pdf":{}}}"#, serde_json::Value::from(outside.display().to_string()));
    crate::core::person::submit(app.core(), recto_agent::parse(&line).expect("a print"));
    app.settle();
    let words = recto_session::SessionError::OutsideQuietRun.message();
    assert_eq!(app.reader().selection_notice.map(|notice| notice.text).as_deref(), Some(words), "its own words");
    assert!(!app.core().tools.printing().busy(), "nothing prepared or left for the host");
    assert!(!outside.exists(), "nothing written");
}

/// Failure: an agent's `print` reaches the person's window or is reached by it: it shows "Preparing pages for printing… [Stop]" there (a person never sees
/// an agent's words), the person's Stop stops it, its file is handed to the person's printer, or it prints a file that asks not to be copied to a PDF
/// without `--anyway`, as the person's print on paper may. An agent's print goes to its end, its own file, with no notice and nothing for the host.
#[test]
fn an_agents_print_is_the_agents_alone() {
    let disk = Disk::new();
    let doc = disk.file("widths.pdf", &widths(2));
    let restricted = disk.file("restricted.pdf", &locked_allowing("", 1, lopdf::encryption::Permissions::PRINTABLE));
    let app = disk.start();
    app.open(&doc);
    app.settle();
    let out = app.disk().files().join("agents.pdf");
    // Stop pressed, and a hold at the first page that lets go at once: either would show on an agent's print only if the window's watch were its.
    app.core().tools.printing().cancel();
    let (held, go) = hold_at(1);
    drop(go);
    let line = format!(r#"{{"cmd":"print","to_pdf":{}}}"#, serde_json::Value::from(out.display().to_string()));
    let answer = app.agent(recto_agent::Level::Full, &line);
    assert!(held.try_recv().is_err(), "the window's watch heard an agent's print's pages");
    assert!(answer.is_ok() && out.exists(), "the person's Stop doesn't stop an agent's print: {answer:?}");
    app.settle();
    assert_eq!(notice(&app), "-", "nothing on the person's window");
    assert_eq!(printed(&app), "none", "nothing for the host to hand to the person's printer");
    *lock(&AT_PAGE) = None;
    app.open(&restricted);
    app.settle();
    let refused = app.recto(
        recto_agent::Level::Full,
        &["print", "--to-pdf", app.disk().files().join("restricted-out.pdf").to_str().expect("a path")],
    );
    assert_eq!(
        refused.expect_err("refused").code,
        recto_agent::Code::COPY_RESTRICTED,
        "an agent's print to a PDF asks for --anyway"
    );
}

/// Failure (S2, found on the way): the person's print of a file that asks not to be copied, which on paper goes ahead and is its pages with no such ask
/// while it's there, can be found by an agent: its file named as v0.1.0 named it (`recto-print-PID-N.pdf` in the temp folder, the process's number being
/// in `status`), so an agent at Control could `open` it while the system's printing had it and draw its pages with no `--anyway` and nothing logged; or
/// its path reaches an agent in an answer. With the file ready for the host: the name v0.1.0 would have given it isn't there to open, and no answer an
/// agent gets while it's there carries the print's file.
#[test]
fn an_agent_cant_find_the_persons_print_file() {
    use recto_agent::Level;
    let disk = Disk::new();
    let restricted = disk.file("restricted.pdf", &locked_allowing("", 2, lopdf::encryption::Permissions::PRINTABLE));
    let app = disk.start();
    app.open(&restricted);
    app.settle();
    // Prepared, and not yet let go by the system's printing: the file is there, as long as the printer takes, with no name.
    let before = unnamed_files_held();
    answer(&app, ALL);
    app.settle();
    let folder = recto_platform::local::system_temp_dir().unwrap_or_else(std::env::temp_dir);
    assert!(
        app.core().tools.printing().has_ready() && unnamed_files_held().len() > before.len(),
        "the print's file is there"
    );
    assert_eq!(print_files_left(), Vec::<String>::new(), "by no name");
    let guessed = folder.join(format!("recto-print-{}-1.pdf", std::process::id()));
    let opened = app.recto(Level::Full, &["open", guessed.to_str().expect("a path")]);
    let mut told = Vec::new();
    for words in [&["status"][..], &["tabs"], &["where"], &["facts"], &["session", "show"], &["sidebar"]] {
        told.push(format!("{words:?}: {:?}", app.recto(Level::Full, words)));
    }
    assert!(opened.is_err(), "the name v0.1.0 would have given the print opened: {opened:?}");
    for line in &told {
        assert!(!line.contains("recto-print-"), "an agent's answer carries the print's file: {line}");
    }
    assert_ne!(printed(&app), "none", "the print went ahead");
}

/// Failure (S2, the coordinator's word on 2026-11-14): a print that fails after its first page, as when a page's file has gone, leaves its half-written file in
/// the temp folder, gives the host a file of the pages it got through, or says nothing. A tab of two files' pages, the second's file taken away once the
/// first page is printed: the refusal's words are said, nothing's for the host, and nothing's left.
#[test]
fn a_print_that_fails_mid_way_leaves_nothing() {
    use recto_agent::Level;
    let disk = Disk::new();
    let (first, second) = (disk.file("first.pdf", &widths(1)), disk.pdf("second.pdf", 1));
    let app = disk.start();
    app.open(&first);
    app.settle();
    app.recto(Level::Full, &["page", "insert", second.to_str().expect("a path"), "--at", "2"])
        .expect("its page added");
    app.settle();
    let (held, go) = hold_at(1);
    answer(&app, ALL);
    held.recv_timeout(WAIT_LIMIT).expect("the print held after its first page");
    std::fs::remove_file(&second).expect("the second file taken away");
    drop(go);
    app.settle();
    let said = notice(&app);
    assert!(said != "-" && !said.contains("Preparing"), "the failure is said: {said}");
    assert_eq!(printed(&app), "none", "nothing for the host");
    assert_eq!(print_files_left(), Vec::<String>::new(), "nothing's left");
}

/// The session held by another thread, as a save or a long read holds it, until the returned sender is dropped: a print's thread can't get past its page
/// list meanwhile.
pub(super) fn session_held(app: &HeadlessApp) -> (std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    let core = std::sync::Arc::clone(app.core());
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let holder = std::thread::Builder::new()
        .name("test-session-holder".into())
        .spawn(move || {
            core.host.session.with_session(|_| {
                held_tx.send(()).expect("the test is waiting");
                let _ = done_rx.recv_timeout(WAIT_LIMIT);
            });
        })
        .expect("a thread");
    held_rx.recv_timeout(WAIT_LIMIT).expect("the session held");
    (done_tx, holder)
}

/// Failure (review of S2, S4): between the panel's answer and the print's thread reaching its first page the window didn't know a print was asked for: ⌘P
/// was enabled again, no "Preparing… [Stop]" showed, and a second panel answered in that gap was taken, so the host kept the second panel's printer and
/// way (Save as PDF among them) for the first print's file. With the session held, so the first print's thread waits: the notice is there from the
/// answer, File > Print is off, a second answer isn't understood, and the one file that comes out carries the first answer's number for the host.
#[test]
fn a_print_is_asked_at_once_and_a_second_answer_meanwhile_is_refused() {
    let disk = Disk::new();
    let doc = disk.file("widths.pdf", &widths(2));
    let app = disk.start();
    app.open(&doc);
    app.settle();
    let tab = lock(&app.core().published).reader.shown_tab;
    let print = |all: bool, to_file: bool, serial: u64| UiAction::Print {
        tab,
        all,
        first: 2,
        last: 2,
        selection_only: false,
        to_file,
        serial,
    };
    let (release, holder) = session_held(&app);
    let first = app.act(print(true, false, 1));
    let shown = app.core().tools.printing().selection_notice();
    let print_enabled = app.core().menu_item_enabled("file.print".into());
    let second = app.act(print(false, true, 2));
    drop(release);
    holder.join().expect("the holder");
    app.settle();
    assert!(first.understood, "the first answer is taken");
    let shown = shown.expect("a notice from the answer on");
    assert!(
        shown.text.starts_with("Preparing") && shown.action_title.as_deref() == Some("Stop"),
        "{shown:?}"
    );
    assert!(!print_enabled, "File > Print is off once a print is asked for");
    assert!(!second.understood, "a second answer meanwhile isn't taken");
    let asked_by = app.core().tools.printing().take_ready().map(|(path, asked_by)| {
        app.core().tools.printing().finished(path.to_str().expect("a path"));
        asked_by
    });
    assert_eq!(
        asked_by,
        Some(1),
        "one file, for the first answer, which the host prints with that answer's printer and way"
    );
    assert_eq!(printed(&app), "none");
}

/// Failure (review of S2, S4): a quit, or Stop, that lands after the panel's answer and before the print's thread begins is undone when it begins, so the
/// print goes on and leaves a file for a host that's quitting. With the session held, the quit comes first: nothing's printed and nothing's left.
#[test]
fn a_quit_before_the_print_begins_stops_it() {
    let disk = Disk::new();
    let doc = disk.file("widths.pdf", &widths(2));
    let app = disk.start();
    app.open(&doc);
    app.settle();
    let tab = lock(&app.core().published).reader.shown_tab;
    let (release, holder) = session_held(&app);
    app.act(UiAction::Print {
        tab,
        all: true,
        first: 1,
        last: 1,
        selection_only: false,
        to_file: false,
        serial: 3,
    });
    app.core().tools.printing().quitting();
    drop(release);
    holder.join().expect("the holder");
    app.settle();
    assert_eq!(printed(&app), "none", "the quit stopped it");
    assert_eq!(print_files_left(), Vec::<String>::new(), "nothing's left");
}

/// Failure (review of S2, S1): an update hands off and quits in the moment between the person's print's drawing ending and its file being the host's (the
/// print looked neither prepared nor ready, and the command was no longer counted), so the print is lost without a word; or the update's check names the
/// person's print as an agent's command. Held there: the check isn't safe, names the print and the person's command and no agent's; once the file's
/// handed over and done with, it's safe.
#[test]
fn a_hand_off_waits_for_the_persons_print_to_be_handed_over() {
    let disk = Disk::new();
    let doc = disk.file("widths.pdf", &widths(1));
    let app = disk.start();
    app.open(&doc);
    app.settle();
    let (told, held) = std::sync::mpsc::channel();
    let (go, waiting) = std::sync::mpsc::channel();
    *lock(&AFTER_END) = Some((told, waiting));
    answer(&app, ALL);
    held.recv_timeout(WAIT_LIMIT).expect("the print held after its drawing ended");
    let check = app.core().hand_off_check();
    drop(go);
    app.settle();
    let codes: Vec<&str> = check.busy.iter().map(|item| item.code.as_str()).collect();
    assert!(!check.safe, "an update hands off with the print's file not yet the host's: {codes:?}");
    assert!(codes.contains(&"printing") && codes.contains(&"person_command"), "{codes:?}");
    assert!(!codes.contains(&"agent_command"), "the person's print named as an agent's command: {codes:?}");
    assert_ne!(printed(&app), "none", "the print's file reached the host");
    app.settle();
    assert!(app.core().hand_off_check().safe, "nothing in flight once it's done");
}

/// Failure (review of S2): the defensive PrintWatch::ready replacement keeps the old descriptor for good. The one-print rule refuses a second answer
/// while ready, so inject the replacement through that actual callback, then prove the first descriptor goes and the replacement's completion holds none.
#[test]
fn a_print_file_the_host_never_took_goes_when_the_next_comes() {
    use std::os::unix::fs::PermissionsExt;

    let disk = Disk::new();
    let doc = disk.file("widths.pdf", &widths(1));
    let app = disk.start();
    app.open(&doc);
    app.settle();
    let before = unnamed_files_held();
    answer(&app, ALL);
    app.settle();
    let first = unnamed_files_held();
    let watch = crate::core::printing::watch(
        app.core().tools.printing_held(),
        std::sync::Arc::clone(&app.core().core),
        std::sync::Arc::downgrade(&app.core().host),
    );
    let replacement = unnamed_file(&widths(1));
    replacement
        .set_permissions(std::fs::Permissions::from_mode(0o600))
        .expect("the print's private mode");
    watch.ready(replacement);
    let after = unnamed_files_held();
    assert_eq!(
        (first.len(), after.len()),
        (before.len() + 1, before.len() + 1),
        "one file held, then the second's alone: {before:?}, {first:?}, {after:?}"
    );
    assert_ne!(first, after, "the first's file went");
    assert_ne!(printed(&app), "none");
    assert_eq!(unnamed_files_held(), before, "nothing's held");
    assert_eq!(print_files_left(), Vec::<String>::new(), "nothing has a name");
}

/// **Failure (S2's follow-up (4)): a print reads its pages when its thread starts,** so a ⌘R pressed in the same instant, after the print was asked, was
/// printed. The print holds the person's turn until its pages are read: the turn waits, and is done after.
#[test]
fn a_turn_asked_after_a_print_isnt_printed() {
    let disk = Disk::new();
    let doc = disk.file("widths.pdf", &widths(2));
    let app = disk.start();
    app.open(&doc);
    app.settle();
    let (held, go) = app.hold_next("print");
    answer(&app, ALL);
    held.recv_timeout(WAIT_LIMIT).expect("the print held before it reads its pages");
    app.act(UiAction::Menu { id: "rotate.right".into() });
    assert!(app.core().core.flush(WAIT_LIMIT), "the session thread took the turn");
    drop(go);
    app.settle();
    let (path, _) = app.core().tools.printing().take_ready().expect("the print's file");
    let bytes = std::fs::read(&path).expect("the print's file");
    app.core().tools.printing().finished(path.to_str().expect("a path"));
    let turned = String::from_utf8_lossy(&bytes).matches("/Rotate 90").count();
    assert_eq!(turned, 0, "printed as the pages were when it was asked");
    assert!(app.core().menu_item_enabled("edit.undo".into()), "and the turn done after it");
}

/// **Failure (the phase's closing read, 2026-11-15): a print parked behind the person's highlight and then dropped from the lane was never answered,** so
/// "Preparing the page for printing. [Stop]" showed for good, every later print said "A print is already being prepared.", and the print's `busy` kept a
/// hand-off refusing, so no update installed. Dropped at the cap, and with what was parked at a quit's patience the person then stayed: the page says the
/// lane's words once, the print is neither being prepared nor busy, a hand-off is free to go, and the next print prints.
#[cfg(unix)]
#[test]
fn a_print_dropped_from_the_lane_leaves_printing_free() {
    use super::persons_lane::{Dropped, parked_then_dropped};
    for how in [Dropped::AtTheCap, Dropped::AtAQuit] {
        let disk = Disk::new();
        let doc = disk.pdf("a.pdf", 2);
        let app = disk.start();
        app.open(&doc);
        app.settle();
        parked_then_dropped(&app, how, || answer(&app, ALL));
        let printing = app.core().tools.printing();
        let check = app.core().hand_off_check();
        assert_eq!(
            (notice(&app), printing.preparing(), printing.busy(), check.busy),
            (format!("{:?} []", how.words()), false, false, Vec::new()),
            "{how:?}: said once, and nothing left under way"
        );
        app.core().dismiss_selection_notice();
        answer(&app, ALL);
        app.settle();
        assert_ne!(printed(&app), "none", "{how:?}: the next print prints");
    }
}

/// Failure: another process's print (a test running beside this one, another run on this Mac) has its unnamed file in the shared temp folder for its
/// moment, and this process's check counts it as a leftover of its own: a test failing for what another process did (the full check of
/// `build/find-key`, 2026-11-15: `closing_the_tab_mid_print_lets_the_key_go` saw `.recto-unnamed-e3775755aa70bd88`, which no process here left). Only
/// this process's own count.
#[test]
fn another_processs_print_file_isnt_counted_as_this_ones() {
    // By name alone, with no file made: a file in the shared temp folder would be seen by the tests beside this one under plain `cargo test`, which runs them as threads of one process.
    let pid = std::process::id();
    let other = format!(".recto-unnamed-{}-{:016x}", pid.wrapping_add(1), recto_platform::local::random_u64());
    assert!(!is_print_file_of(&other, pid), "another process's print file counted as this one's");
    assert!(
        is_print_file_of(&format!(".recto-unnamed-{pid}-{:016x}", recto_platform::local::random_u64()), pid),
        "this process's own print file wasn't seen"
    );
}

/// Failure (2026-11-25, `[] then []` in `a_persons_print_does_what_it_did` on a loaded machine): the look at this process's descriptors swallowed every
/// error (a folder that couldn't be listed, an entry that couldn't be read), so one transient failure gave an empty list that the print's check read as
/// "the print's file never was there". A look that fails panics and names the OS error.
#[test]
fn a_look_at_the_descriptors_that_fails_says_so() {
    let folder = recto_testpdf::TempDir::new("fd-look-");
    let missing = folder.join("nothing-here");
    let regular = folder.join("a-file");
    std::fs::write(&regular, b"x").expect("a file");
    for (what, path, os_error) in [("missing", missing, "No such file"), ("a regular file", regular, "Not a directory")] {
        let said = std::panic::catch_unwind(|| unnamed_files_held_in(&path)).expect_err(&format!("{what}: the look panics"));
        let said = said
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| said.downcast_ref::<&str>().map(|s| (*s).to_string()))
            .unwrap_or_default();
        assert!(said.contains(os_error), "{what}: the panic names the OS error, got {said:?}");
    }
}

/// Failure (found while diagnosing `[] then []`, 2026-11-25): "Print anyway" pressed in the moment between the menu's look at a hand-off (not frozen) and
/// the person's lane's (frozen) was refused by the lane, but the row ignored that and left the print counted as under way: the window said "Preparing...",
/// the app's settle never returned and every later print was refused as one being prepared, until the app restarted. The row is run past the menu's look,
/// as that moment would: nothing's being prepared, and once it's not frozen the next print prints.
#[test]
fn a_print_anyway_refused_in_a_hand_off_leaves_printing_free() {
    let disk = Disk::new();
    let restricted = disk.file("restricted.pdf", &locked_allowing("", 2, lopdf::encryption::Permissions::PRINTABLE));
    let app = disk.start();
    app.open(&restricted);
    app.settle();
    answer(&app, Panel { to_file: true, ..ALL });
    app.settle();
    let anyway = app.reader().selection_notice.and_then(|notice| notice.action_id).expect("Print anyway");
    app.core().core.frozen.store(true, std::sync::atomic::Ordering::SeqCst);
    let parts = crate::core::row_parts::RowParts::of(app.core());
    let row = crate::core::menu_rows::row_in(&parts, &anyway).expect("the row");
    crate::core::menu_rows::by_person(&parts, row);
    let printing = app.core().tools.printing();
    assert!(!printing.preparing(), "nothing's being prepared: the command wasn't taken");
    app.core().core.frozen.store(false, std::sync::atomic::Ordering::SeqCst);
    answer(&app, ALL);
    app.settle();
    assert_eq!(printed(&app), "[612,612]", "the next print isn't refused");
}
