//! **The person's File > Print, from the print panel's answer to the file handed to the system's printing** (S2's guard, put in before the person's print
//! became the `print` command run as theirs): which pages each answer prints, the notices said and their buttons, Stop mid-way, a second print asked while
//! one is being prepared, and a file that asks not to be copied, on paper and to a file, with "Print anyway".

use super::files::locked_allowing;
use crate::core::controls::UiAction;
use crate::core::printing::AT_PAGE;
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

/// **The panel answered, as the host answers it:** the tab showing and what was chosen. On `main` before S2 this is the bridge's `print_chosen`, word for
/// word but for its reach into the core: it moves to the one way a person's action comes in with the change.
fn answer(app: &HeadlessApp, panel: Panel) {
    use crate::core::printing::{Request, chosen_range, start};
    let inner = app.core();
    let tab = lock(&inner.published).reader.shown_tab;
    if tab == 0 || inner.core.frozen.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let engine = inner.panes.reading_engine(&inner.engine);
    let pages = engine.points().page_count();
    let selection = panel
        .selection_only
        .then(|| engine.selection().hot())
        .flatten()
        .and_then(|(_, range)| range)
        .map(|range| (range.start.page, range.end.page));
    let Some(range) = chosen_range(pages, panel.all, panel.first, panel.last, selection) else {
        return;
    };
    start(
        inner,
        Request {
            tab: recto_session::TabId(tab),
            range,
            dpi: recto_agent::print::DEFAULT_DPI,
            to_file: panel.to_file,
            anyway: false,
        },
    );
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
    let Some(path) = app.core().tools.printing().take_ready() else {
        return "none".into();
    };
    let bytes = std::fs::read(&path).expect("the print's file");
    let text = String::from_utf8_lossy(&bytes);
    let widths: Vec<&str> = text
        .match_indices("/MediaBox [0 0 ")
        .filter_map(|(at, found)| text[at + found.len()..].split(' ').next())
        .collect();
    app.core().tools.printing().finished(path.to_str().expect("a path"));
    assert!(!path.exists(), "the print's file goes once the system's printing is done with it");
    format!("[{}]", widths.join(","))
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
    *lock(&AT_PAGE) = Some((page, told, waiting));
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
}
