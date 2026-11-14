//! **The person's File > Print, from the print panel's answer to the file handed to the system's printing** (S2's guard, put in before the person's print
//! became the `print` command run as theirs): which pages each answer prints, the notices said and their buttons, Stop mid-way, a second print asked while
//! one is being prepared, and a file that asks not to be copied, on paper and to a file, with "Print anyway".

use super::files::locked_allowing;
use crate::core::controls::UiAction;
use crate::core::printing::{AFTER_END, AT_PAGE};
use crate::lock;
use crate::testkit::headless::{Disk, HeadlessApp, WAIT_LIMIT};

/// What the print panel answered: all the pages or a range from 1, the selection's pages, and whether it prints to a file (a PDF saved or previewed).
#[derive(Clone, Copy)]
struct Panel {
    all: bool,
    first: u32,
    last: u32,
    selection_only: bool,
    to_file: bool,
}

const ALL: Panel = Panel {
    all: true,
    first: 1,
    last: 1,
    selection_only: false,
    to_file: false,
};

/// **The panel answered, as the host answers it:** the tab showing and what was chosen, through the one way a person's action comes in.
fn answer(app: &HeadlessApp, panel: Panel) {
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
fn notice(app: &HeadlessApp) -> String {
    match app.reader().selection_notice {
        None => "-".into(),
        Some(notice) => format!("{:?} [{}]", notice.text, notice.action_title.unwrap_or_default()),
    }
}

/// **The file the print made, taken as the host takes it and let go as the system's printing does:** the width in points of each page in it, which says
/// which pages of the document it holds, as each page of the test files has its own width. Its file is new and the user's alone while it's there, and it
/// goes when it's let go. "none" if there's no file for the host.
fn printed(app: &HeadlessApp) -> String {
    let Some((path, _)) = app.core().tools.printing().take_ready() else {
        return "none".into();
    };
    let bytes = std::fs::read(&path).expect("the print's file");
    let text = String::from_utf8_lossy(&bytes);
    let widths: Vec<&str> = text
        .match_indices("/MediaBox [0 0 ")
        .filter_map(|(at, found)| text[at + found.len()..].split(' ').next())
        .collect();
    // The user's own: readable and writable by them alone, in their own temp folder, which only they can open.
    let (file_mode, folder_mode, owner) = {
        use std::os::unix::fs::MetadataExt;
        let (file, folder) = (
            std::fs::metadata(&path).expect("the file"),
            std::fs::metadata(path.parent().expect("its folder")).expect("its folder"),
        );
        (file.mode() & 0o777, folder.mode() & 0o077, file.uid())
    };
    assert_eq!((file_mode, folder_mode, owner), (0o600, 0, own_uid()), "{} is the user's alone", path.display());
    app.core().tools.printing().finished(path.to_str().expect("a path"));
    assert!(!path.exists(), "the print's file goes once the system's printing is done with it");
    format!("[{}]", widths.join(","))
}

/// This process's user.
fn own_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    let probe = recto_testpdf::TempDir::new("uid-");
    std::fs::metadata(probe.path()).expect("a folder of ours").uid()
}

/// The print files of this process's in the system's temp folder, made or half made: what a stopped print must not leave.
fn print_files_left() -> Vec<String> {
    let folder = recto_platform::local::system_temp_dir().unwrap_or_else(std::env::temp_dir);
    let pid = std::process::id();
    std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with(&format!("recto-print-{pid}-")) || name.starts_with(&format!(".recto-{pid}-")))
                .collect()
        })
        .unwrap_or_default()
}

/// A print held once it's this many pages in, till the test lets it go: the sender that lets it go.
fn hold_at(page: u32) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
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
    held.recv_timeout(WAIT_LIMIT).expect("the print held at its first page");
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
    held.recv_timeout(WAIT_LIMIT).expect("the print held at its first page");
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
    // Prepared, and not yet let go by the system's printing: the file is there, as long as the printer takes.
    answer(&app, ALL);
    app.settle();
    let folder = recto_platform::local::system_temp_dir().unwrap_or_else(std::env::temp_dir);
    assert!(
        app.core().tools.printing().has_ready() && !print_files_left().is_empty(),
        "the print's file is there"
    );
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
fn session_held(app: &HeadlessApp) -> (std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>) {
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

/// Failure (review of S2): a print's file the host never took is left in the temp folder for good when the next print's file takes its place, as two
/// prints answered in a row with nothing handing the first to the system did. The first goes as the second comes; once the second is done with, nothing's
/// left.
#[test]
fn a_print_file_the_host_never_took_goes_when_the_next_comes() {
    let disk = Disk::new();
    let doc = disk.file("widths.pdf", &widths(1));
    let app = disk.start();
    app.open(&doc);
    app.settle();
    answer(&app, ALL);
    app.settle();
    let first = print_files_left();
    answer(&app, ALL);
    app.settle();
    let after = print_files_left();
    assert_eq!(after.len(), 1, "one file, the second's: {first:?} then {after:?}");
    assert!(
        first.iter().all(|name| !after.contains(name)),
        "the first's file went: {first:?} then {after:?}"
    );
    assert_ne!(printed(&app), "none");
    assert_eq!(print_files_left(), Vec::<String>::new(), "nothing's left");
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
