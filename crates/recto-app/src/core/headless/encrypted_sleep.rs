//! **What keeps a document's pages out of a sleep file is that a password was needed to open it,** not that its file is encrypted on disk, on the app's own
//! core: a file encrypted with an empty user password (permissions only, as `un_report_un_sdg2023.pdf` in the corpus is) opens with no password, has nothing a
//! sleep file could give away, and has one like any other; a file that needed the person's password never has one, before or after it's typed. And a
//! password is for the tab it was typed for: a page added to another document from a file only the renderer needs one for is never drawn or read with
//! it, the key ends with its tab, and that page is one that couldn't be read to Find and an agent's search.

use super::files::locked_allowing;
use crate::testkit::headless::{Disk, HeadlessApp};
use lopdf::encryption::Permissions;
use recto_platform::slow::Seam;

fn full(app: &HeadlessApp, words: &[&str]) -> serde_json::Value {
    app.recto(recto_agent::Level::Full, words)
        .unwrap_or_else(|error| panic!("{words:?}: {error:?}"))
}

fn tab_named(app: &HeadlessApp, name: &str) -> u64 {
    app.session_json()["tabs"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|tab| tab["path"].as_str().is_some_and(|path| path.ends_with(name)))
        .and_then(|tab| tab["id"].as_u64())
        .expect("the tab")
}

/// Every sleep file in the state folder, read back: how many pictures each has.
fn sleep_files(app: &HeadlessApp) -> Vec<usize> {
    let mut folders = vec![app.disk().paths().root.clone()];
    let mut found = Vec::new();
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else if entry.file_name().to_string_lossy().starts_with("sleep-") {
                let bytes = std::fs::read(&path).expect("a sleep file");
                let sleep = recto_session::sleep::decode_sleep(&bytes, recto_session::record::CURRENT_VERSION).expect("a sleep file that reads");
                found.push(sleep.pictures.len());
            }
        }
    }
    found
}

/// The view the host would have: a Retina window, the page at its fit.
fn window_view() -> recto_reader::Viewport {
    recto_reader::Viewport {
        x: 0.0,
        y: 0.0,
        width: 1168.0,
        height: 900.0,
        frame_width: 1168.0,
        magnification: 1.0,
        backing: 2.0,
        live: false,
        scrolled_to: u64::MAX,
    }
}

/// Draws what's in a window's view, as the host would with its UI thread, till it's at rest.
fn drawn(app: &HeadlessApp) {
    let mut view = window_view();
    crate::testkit::headless::applied_as_the_host(&app.core().engine.set_viewport(view));
    crate::testkit::headless::host_turns(app, &mut view, true);
}

/// Opens a file as the person does, typing `password` into the sheet when it's asked for (`None`: the sheet's left unanswered), and draws it.
fn opened_by_the_person(app: &HeadlessApp, file: &std::path::Path, password: Option<&str>) {
    app.act(crate::core::controls::UiAction::Open {
        path: file.display().to_string(),
    });
    app.settle();
    let prompt = app.reader().password_prompt;
    match (prompt, password) {
        (Some(prompt), Some(password)) => {
            app.core()
                .tools
                .prompts()
                .password_answered(prompt.id, Some(password.into()), &app.core().core, &app.core().window);
            app.settle();
            assert!(app.reader().password_prompt.is_none(), "the password opened it");
        }
        (Some(_), None) => return,
        (None, None) => {}
        (None, Some(_)) => panic!("{file:?} was to ask for its password"),
    }
    drawn(app);
}

/// Failure (the sleep-costs measurement of 2026-11-13: `un_report_un_sdg2023.pdf` slept three times in two runs, every sleep counter 0, no error): a PDF
/// encrypted with an empty user password, which opens with no password asked, never had a sleep file, not even its facts, so it always woke bare, and the
/// refusal was counted nowhere. The sleep rule took `encrypted` for "protected", where opening.md and the agents' gate already told the two apart. Opened with
/// no password, drawn, left and put to sleep, it has a sleep file with a picture, and wakes from it.
#[test]
fn an_encrypted_pdf_anyone_may_open_gets_a_sleep_file_with_pictures() {
    let disk = Disk::new();
    let open_to_anyone = disk.file("open-to-anyone.pdf", &locked_allowing("", 3, Permissions::PRINTABLE));
    let other = disk.pdf("other.pdf", 2);
    let app = disk.start();
    app.core().engine.waking().window_up();
    opened_by_the_person(&app, &open_to_anyone, None);
    opened_by_the_person(&app, &other, None);
    assert!(sleep_files(&app).is_empty(), "nothing slept yet");
    full(&app, &["session", "sleep", "--tab", &format!("#{}", tab_named(&app, "open-to-anyone.pdf"))]);
    app.settle();
    let files = sleep_files(&app);
    assert!(
        files.len() == 1 && files[0] >= 1,
        "put to sleep, it has a sleep file with a picture: {files:?}; {}",
        app.record_json()["sleep"]
    );
    full(&app, &["tap", &format!("#{}", tab_named(&app, "open-to-anyone.pdf"))]);
    drawn(&app);
    let woke = app.core().engine.report().0["last_wake"].clone();
    assert_eq!(woke["had"], "sleep_file", "it wakes from it: {woke}");
    drop(app.quit());
}

/// Failure, the other side of the same rule: a file that needed the person's password has its pages written to disk in the clear, in a sleep file, once it's
/// been opened with the password and put to sleep, or while it waits for a password nobody's typed. Both are drawn (the one with its password) and left, and
/// every tab off screen is put to sleep by memory pressure, as an agent may not ask for a protected tab's sleep: the plain one beside them has its sleep
/// file, so files are being written, and there's none with more than the plain one's pictures. A page from such a file can't be added to another document
/// at all: it needs the password, and the loader that adds pages is given none.
#[test]
fn a_pdf_that_needed_a_password_never_gets_a_sleep_file() {
    let disk = Disk::new();
    let typed = disk.file("typed.pdf", &locked_allowing("right", 3, Permissions::all()));
    let unanswered = disk.file("unanswered.pdf", &locked_allowing("right", 2, Permissions::all()));
    let plain = disk.pdf("plain.pdf", 2);
    let other = disk.pdf("other.pdf", 2);
    let app = disk.start();
    app.core().engine.waking().window_up();
    opened_by_the_person(&app, &typed, Some("right"));
    opened_by_the_person(&app, &unanswered, None);
    opened_by_the_person(&app, &plain, None);
    let refused = app
        .recto(recto_agent::Level::Full, &["page", "insert", typed.to_str().unwrap(), "--at", "1"])
        .expect_err("a page from a file that needs a password isn't added");
    assert_eq!(
        (refused.code.as_str(), refused.exit),
        ("password_needed", recto_agent::exit::PROTECTED),
        "and it's refused for needing the password, not for something else: {refused:?}"
    );
    opened_by_the_person(&app, &other, None);
    app.core().qa_session_verb("memory_pressure", "warning").expect("a QA verb").expect("pressure");
    app.settle();
    app.wait_until("the queue written", || app.record_json()["sleep"]["leaving"]["writer"] == false);
    let sleep = app.record_json()["sleep"].clone();
    assert!(sleep["gone_dormant"]["pressure"].as_u64().unwrap_or(0) >= 2, "{sleep}");
    let files = sleep_files(&app);
    assert_eq!(
        files.len(),
        1,
        "the plain tab's alone, none for either file that needed a password: {files:?}; {sleep}"
    );
    drop(app.quit());
}

/// Failure (found with the one above, 2026-11-13): a page added to a plain document from a file anyone may open, encrypted or not, was counted as a protected
/// file's page, so the whole document's sleep file had no pictures and it woke bare. Added at the front and drawn, then left and put to sleep: its sleep file
/// has a picture. (A file that needs a password can't be added at all, which `a_pdf_that_needed_a_password_never_gets_a_sleep_file` holds.)
#[test]
fn a_page_added_from_an_encrypted_pdf_anyone_may_open_keeps_the_documents_pictures() {
    let disk = Disk::new();
    let open_to_anyone = disk.file("open-to-anyone.pdf", &locked_allowing("", 1, Permissions::PRINTABLE));
    let plain = disk.pdf("plain.pdf", 2);
    let other = disk.pdf("other.pdf", 2);
    let app = disk.start();
    app.core().engine.waking().window_up();
    opened_by_the_person(&app, &plain, None);
    full(&app, &["page", "insert", open_to_anyone.to_str().unwrap(), "--at", "1"]);
    drawn(&app);
    assert_eq!(full(&app, &["edits"])["pages"], 3);
    opened_by_the_person(&app, &other, None);
    full(&app, &["session", "sleep", "--tab", &format!("#{}", tab_named(&app, "plain.pdf"))]);
    app.settle();
    let files = sleep_files(&app);
    assert!(
        files.len() == 1 && files[0] >= 1,
        "the document with the added page has a sleep file with a picture: {files:?}; {}",
        app.record_json()["sleep"]
    );
    drop(app.quit());
}

/// A file the two readers see differently: [`locked_allowing`]'s, opened only with `password` by PDFium, with a newer section on the end whose trailer
/// doesn't name the `/Encrypt` the older one does. lopdf reads the newest trailer and the file as plain, so the loader that adds pages finds no password
/// needed; PDFium honours the older one and asks for it. A crafted file can do this, and nothing else needing a password can be added to a document.
fn locked_for_one_reader(password: &str) -> Vec<u8> {
    let mut bytes = locked_allowing(password, 1, Permissions::all());
    let number_after = |key: &[u8]| -> String {
        let at = bytes.windows(key.len()).rposition(|window| window == key).expect("the key") + key.len();
        String::from_utf8_lossy(&bytes[at..])
            .split(|c: char| !c.is_ascii_digit() && c != ' ')
            .next()
            .unwrap()
            .trim()
            .to_string()
    };
    let (size, root, prev) = (number_after(b"/Size "), number_after(b"/Root "), number_after(b"startxref\n"));
    let here = bytes.len();
    bytes.extend(
        format!(
            "xref\n0 1\n0000000000 65535 f \ntrailer\n<< /Size {size} /Root {root} R /Prev {prev} /ID [(0123456789abcdef) (0123456789abcdef)] >>\nstartxref\n{here}\n%%EOF\n"
        )
        .as_bytes(),
    );
    bytes
}

/// What only the page's own note says, of a page added from a file that needs a password: the document-level "This PDF needs a password." doesn't.
const ADDED_PAGES_NOTE: &str = "comes from a PDF that needs a password";

/// Opens `file` as the person opens it, a file only PDFium needs a password for: the loader reads it as plain, the page view's worker finds it needs one,
/// which is asked for and typed, and its own tab draws with it.
fn opened_for_one_reader(app: &HeadlessApp, file: &std::path::Path, password: &str) {
    app.act(crate::core::controls::UiAction::Open {
        path: file.display().to_string(),
    });
    app.settle();
    drawn(app);
    app.settle();
    let prompt = app.reader().password_prompt.expect("its password is asked for, as PDFium needs one");
    app.core()
        .tools
        .prompts()
        .password_answered(prompt.id, Some(password.into()), &app.core().core, &app.core().window);
    app.settle();
    drawn(app);
    assert!(app.reader().password_prompt.is_none(), "the password opened it");
    assert_eq!(app.core().engine.note(), None, "its own tab draws with the password");
}

/// Opens the plain document, adds `added`'s page at its front, as the loader reads it as plain, and brings that page into view, drawn.
fn added_and_in_view(app: &HeadlessApp, plain: &std::path::Path, added: &std::path::Path) {
    opened_by_the_person(app, plain, None);
    let inserted = app.recto(recto_agent::Level::Full, &["page", "insert", added.to_str().unwrap(), "--at", "1"]);
    assert!(inserted.is_ok(), "the loader reads it as plain, so it's added: {inserted:?}");
    // The view keeps the reader's place, now page 2: the added page is brought into view, where a sleep file would picture it.
    full(app, &["goto", "1"]);
    drawn(app);
}

/// Every sleep file in the state folder, read back: the pages each has a picture of.
fn pictured_pages(app: &HeadlessApp) -> Vec<Vec<u32>> {
    let mut folders = vec![app.disk().paths().root.clone()];
    let mut found = Vec::new();
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else if entry.file_name().to_string_lossy().starts_with("sleep-") {
                let sleep = recto_session::sleep::decode_sleep(&std::fs::read(&path).expect("a sleep file"), recto_session::record::CURRENT_VERSION)
                    .expect("a sleep file that reads");
                found.push(sleep.pictures.iter().map(|picture| picture.page).collect());
            }
        }
    }
    found
}

/// Failure (review round 1 of the measured faults, S1): a password was used for every file a page view's worker opened, an added PDF's too. A crafted
/// file lopdf reads with no password while PDFium asks for one is opened by the person, who types its password; the same file is added to a plain
/// document, as the loader finds no password needed; that document's page from it was drawn with the person's password, pictured in its sleep file,
/// and on screen for an agent to look at. A password is for the tab it was typed for: the page in the plain document says why it isn't shown and draws
/// nothing, its sleep file has no picture of it, and its own pages draw as ever.
#[test]
fn a_page_added_from_a_file_only_one_reader_needs_a_password_for_is_never_drawn_with_it() {
    let disk = Disk::new();
    let crafted = disk.file("crafted.pdf", &locked_for_one_reader("right"));
    let plain = disk.pdf("plain.pdf", 2);
    let other = disk.pdf("other.pdf", 2);
    let app = disk.start();
    app.core().engine.waking().window_up();
    opened_for_one_reader(&app, &crafted, "right");
    added_and_in_view(&app, &plain, &crafted);
    assert!(
        app.core().engine.note().is_some_and(|note| note.contains(ADDED_PAGES_NOTE)),
        "the plain document's page from it says why it isn't shown: {:?}; {}; {}",
        app.core().engine.note(),
        app.core().engine.report().0,
        full(&app, &["edits"])
    );
    assert!(app.reader().password_prompt.is_none(), "and the plain document isn't asked for a password");
    opened_by_the_person(&app, &other, None);
    full(&app, &["session", "sleep", "--tab", &format!("#{}", tab_named(&app, "plain.pdf"))]);
    app.settle();
    let pictured = pictured_pages(&app);
    assert_eq!(
        pictured.len(),
        1,
        "the plain document's sleep file alone: {pictured:?}; {}",
        app.record_json()["sleep"]
    );
    assert!(!pictured[0].contains(&0), "with no picture of the page from the crafted file: {pictured:?}");
    drop(app.quit());
}

/// The crafted file opened by the person with its password typed (tab A), and a plain document (tab B) with a page added from it, in view: the setup of
/// the two tests below. Says A's id, and the pids of the workers the page view had for A's file while A was showing.
fn typed_for_a_and_added_to_b(app: &HeadlessApp, crafted: &std::path::Path, plain: &std::path::Path) -> (u64, Vec<u32>) {
    app.core().engine.waking().window_up();
    let before: Vec<u32> = app.core().engine.workers().pids();
    opened_for_one_reader(app, crafted, "right");
    let crafted_workers: Vec<u32> = app.core().engine.workers().pids().into_iter().filter(|pid| !before.contains(pid)).collect();
    assert!(!crafted_workers.is_empty(), "A's file has a worker, given the password");
    let a = tab_named(app, "crafted.pdf");
    added_and_in_view(app, plain, crafted);
    assert!(
        app.core().engine.note().is_some_and(|note| note.contains(ADDED_PAGES_NOTE)),
        "B's page from it says why it isn't shown: {:?}",
        app.core().engine.note()
    );
    (a, crafted_workers)
}

/// Failure (the second review of the measured faults): a renderer given the password typed for tab A was dropped only when no hot or warm tab wanted its
/// path, and a document with a page added from that file wants it. So with A closed the keyring let the key go, but the renderer kept it, its worker
/// kept its own copy, and a worker it started again was given it. Closing A while B is showing: the key goes from everywhere in the process (nothing
/// holds it, so its bytes were overwritten as the last holder let go), and the worker that had it is gone.
#[test]
fn closing_the_tab_a_password_was_typed_for_ends_every_copy_of_it_though_a_document_adds_its_file() {
    let disk = Disk::new();
    let crafted = disk.file("crafted.pdf", &locked_for_one_reader("right"));
    let plain = disk.pdf("plain.pdf", 2);
    let app = disk.start();
    let (a, crafted_workers) = typed_for_a_and_added_to_b(&app, &crafted, &plain);
    let key = std::sync::Arc::downgrade(&app.core().keys.for_path(&crafted).expect("the key typed for A is held"));
    full(&app, &["close", "--tab", &format!("#{a}")]);
    app.settle();
    drawn(&app);
    app.wait_until("no holder of the key typed for A", || key.upgrade().is_none());
    assert!(app.core().keys.is_empty(), "the keyring holds no key: no tab owns one");
    for pid in crafted_workers {
        app.wait_until("the worker given the key gone", || recto_platform::isolate::process_memory(pid).is_none());
        assert!(!app.core().engine.workers().pids().contains(&pid), "nor listed as the page view's");
    }
    drop(app.quit());
}

/// Failure (the second review of the measured faults): with A closed and its key let go, the renderer given it stayed for B, which adds A's file. The
/// person opens that file again: no key is held, so the loader reads it as plain and the new tab isn't marked as protected, and its jobs, being its own
/// file's, drew on the stale renderer with the old password: nobody was asked for it, and an agent could screenshot the tab and read its text. Opened
/// again, it asks for its password, it's withheld from agents till it's typed, nothing of it draws meanwhile, and B's page from it still says why it
/// isn't shown.
#[test]
fn a_file_opened_again_after_the_tab_its_password_was_typed_for_closed_asks_for_it_again() {
    let disk = Disk::new();
    let crafted = disk.file("crafted.pdf", &locked_for_one_reader("right"));
    let plain = disk.pdf("plain.pdf", 2);
    let app = disk.start();
    let (a, _) = typed_for_a_and_added_to_b(&app, &crafted, &plain);
    full(&app, &["close", "--tab", &format!("#{a}")]);
    app.settle();
    drawn(&app);
    let b = tab_named(&app, "plain.pdf");
    app.act(crate::core::controls::UiAction::Open {
        path: crafted.display().to_string(),
    });
    app.settle();
    drawn(&app);
    app.settle();
    let again = tab_named(&app, "crafted.pdf");
    assert!(app.reader().password_prompt.is_some(), "its password is asked for again");
    let refused = app
        .recto(recto_agent::Level::Full, &["text", "--tab", &format!("#{again}"), "--page", "1"])
        .expect_err("an agent reads nothing of it");
    assert_eq!(refused.exit, recto_agent::exit::PROTECTED, "withheld as protected: {refused:?}");
    let report = app.core().engine.report().0;
    assert_eq!(report["tab"], again, "{report}");
    assert_eq!(
        report["tiles"]["attached"], 0,
        "nothing of it is drawn while it waits for its password: {report}"
    );
    full(&app, &["tap", &format!("#{b}")]);
    full(&app, &["goto", "1"]);
    drawn(&app);
    assert!(
        app.core().engine.note().is_some_and(|note| note.contains(ADDED_PAGES_NOTE)),
        "B's page from it still says why it isn't shown: {:?}",
        app.core().engine.note()
    );
    drop(app.quit());
}

/// Failure (a whole-suite run of 2026-11-14: the test above, once in 484, "its password is asked for again"): a renderer given the password typed for
/// tab A was kept while any tab the page view held had its file as its own, not only A. Its thread lets renderers go between jobs, so with a job of
/// another tab's under way as A was closed, and the file opened again before that job ended, the new tab was held by then and kept A's renderer: it
/// drew with A's password, nobody was asked for it, and an agent could screenshot it. Forced with the page view's thread held in a job for B
/// (`Seam::Worker`) while A is closed and its file opened again: the new tab asks for its password and nothing of it is drawn meanwhile.
#[test]
fn a_file_opened_again_while_the_page_view_was_busy_as_its_tab_closed_asks_for_its_password() {
    let disk = Disk::new();
    let crafted = disk.file("crafted.pdf", &locked_for_one_reader("right"));
    let plain = disk.pdf("plain.pdf", 2);
    let app = disk.start();
    let (a, _) = typed_for_a_and_added_to_b(&app, &crafted, &plain);
    recto_platform::slow::gate::hold(&[Seam::Worker]);
    full(&app, &["goto", "3"]);
    crate::testkit::headless::host_turns(&app, &mut window_view(), false);
    app.wait_until("a job for B held", || app.waiting(Seam::Worker) > 0);
    // The page view's thread stays held in B's job throughout, so what's waited for here is the session's part and the host's, not the page view at rest,
    // which the kit's settle waits for (`crate::settled`).
    full(&app, &["close", "--tab", &format!("#{a}")]);
    crate::testkit::headless::host_turns(&app, &mut window_view(), false);
    app.act(crate::core::controls::UiAction::Open {
        path: crafted.display().to_string(),
    });
    crate::testkit::headless::host_turns(&app, &mut window_view(), false);
    assert!(app.waiting(Seam::Worker) >= 1, "the page view's thread still held in B's job");
    recto_platform::slow::gate::open(Seam::Worker);
    drawn(&app);
    app.settle();
    let again = tab_named(&app, "crafted.pdf");
    let report = app.core().engine.report().0;
    assert!(app.reader().password_prompt.is_some(), "opened again, its password is asked for: {report}");
    assert_eq!(report["tab"], again, "{report}");
    assert_eq!(
        report["tiles"]["attached"], 0,
        "nothing of it is drawn with the password typed for the tab that closed: {report}"
    );
    drop(app.quit());
}

/// A plain document of two pages with the crafted file's page added between them, at page 2: every page says "page", the added one "Locked page 1".
fn plain_with_the_crafted_page_in_the_middle(disk: Disk) -> HeadlessApp {
    let crafted = disk.file("crafted.pdf", &locked_for_one_reader("right"));
    let plain = disk.pdf("plain.pdf", 2);
    let app = disk.start();
    opened_by_the_person(&app, &plain, None);
    full(&app, &["page", "insert", crafted.to_str().unwrap(), "--at", "2"]);
    assert_eq!(full(&app, &["edits"])["pages"], 3);
    app
}

/// Failure (the second review of the measured faults): the person's Find over a plain document with a page added from a file only the renderer needs a
/// password for stopped dead at that page: "This PDF needs a password." though the document needs none, the hits before it shown and nothing after it
/// searched. A page that needs a password, from a file that isn't the document's own, is a page that couldn't be read, as the page view has it: skipped,
/// counted, and said as such, and the search goes on.
#[test]
fn find_in_a_plain_document_goes_past_a_page_added_from_a_file_that_needs_a_password() {
    let disk = Disk::new();
    let app = plain_with_the_crafted_page_in_the_middle(disk);
    app.act(crate::core::controls::UiAction::FindTyped { text: "page".into() });
    app.settle();
    app.wait_until("Find ended", || {
        let view = crate::core::find::view(app.core());
        !view.running && !view.status.is_empty()
    });
    let view = crate::core::find::view(app.core());
    let pages: Vec<String> = crate::core::find::rows(app.core(), 0, view.row_count)
        .into_iter()
        .filter(|row| row.kind == crate::find::FindRowKind::Page)
        .map(|row| row.label)
        .collect();
    assert_eq!(pages.len(), 2, "hits on the pages before and after it, none on it: {pages:?}; {}", view.status);
    assert!(pages[0].contains('1') && pages[1].contains('3'), "{pages:?}");
    assert!(
        view.status.ends_with("1 page couldn't be read"),
        "says what's true, as for any page that couldn't be read: {}",
        view.status
    );
    assert!(!view.status.contains("password"), "{}", view.status);
    drop(app.quit());
}

/// Failure, the agent's side of the same: `search` over that document exited `needs_password` on a document it may read, and `text` on the added page
/// said the document needed one. `search` returns the hits before and after it, with the page among those that couldn't be read; `text` of that page says
/// it couldn't be read, as for any page that can't be, and of the others reads them.
#[test]
fn an_agents_search_and_text_in_a_plain_document_go_past_a_page_added_from_a_file_that_needs_a_password() {
    let disk = Disk::new();
    let app = plain_with_the_crafted_page_in_the_middle(disk);
    let found = full(&app, &["search", "page"]);
    let pages: Vec<u64> = found["hits"].as_array().unwrap().iter().filter_map(|hit| hit["page"].as_u64()).collect();
    assert_eq!(pages, [1, 3], "{found}");
    assert_eq!(found["coverage"]["failed_pages"], serde_json::json!([2]), "{found}");
    let refused = app
        .recto(recto_agent::Level::Full, &["text", "--page", "2"])
        .expect_err("the added page can't be read");
    assert_eq!(refused.code, "page_unreadable", "{refused:?}");
    assert_eq!(full(&app, &["text", "--page", "3"])["untrusted"]["text"], "plain.pdf page 2");
    drop(app.quit());
}
