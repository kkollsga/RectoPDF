//! **What keeps a document's pages out of a sleep file is that a password was needed to open it,** not that its file is encrypted on disk, on the app's own
//! core: a file encrypted with an empty user password (permissions only, as `un_report_un_sdg2023.pdf` in the corpus is) opens with no password, has nothing a
//! sleep file could give away, and has one like any other; a file that needed the person's password never has one, before or after it's typed.

use super::files::locked_allowing;
use crate::testkit::headless::{Disk, HeadlessApp};
use lopdf::encryption::Permissions;

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

/// Stands in for the host's UI thread, as `sleeping.rs`'s does: takes what the page view says, until nothing's queued and everything in view is drawn. Bounded.
fn drawn(app: &HeadlessApp) {
    let engine = &app.core().engine;
    let mut view = window_view();
    crate::testkit::headless::applied_as_the_host(&engine.set_viewport(view));
    let began = std::time::Instant::now();
    loop {
        assert!(
            app.core().core.flush(crate::testkit::headless::WAIT_LIMIT),
            "the session thread took what was queued"
        );
        let mut ops = engine.drain();
        crate::testkit::headless::applied_as_the_host(&ops);
        engine.applied();
        let mut quiet = ops.is_empty();
        while let Some(op) = ops.pop() {
            if let recto_reader::Op::ScrollTo { y, seq } = op {
                view.y = y;
                view.scrolled_to = seq;
                ops = engine.set_viewport(view);
                crate::testkit::headless::applied_as_the_host(&ops);
                engine.applied();
                quiet = false;
            }
        }
        if quiet && engine.wait_settled(std::time::Duration::from_millis(20)) && engine.report().0["sleep_draws"]["under_way"] == false {
            return;
        }
        assert!(
            began.elapsed() < crate::testkit::headless::WAIT_LIMIT,
            "the page view didn't come to rest: {}",
            engine.report().0
        );
    }
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
            app.core().password_answered(prompt.id, Some(password.into()));
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
    let refused = app.recto(recto_agent::Level::Full, &["page", "insert", typed.to_str().unwrap(), "--at", "1"]);
    assert!(refused.is_err(), "a page from a file that needs a password isn't added: {refused:?}");
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
    // Opened as the person opens it: the loader reads it as plain, and the page view's worker finds it needs a password, which is asked for and typed.
    app.act(crate::core::controls::UiAction::Open {
        path: crafted.display().to_string(),
    });
    app.settle();
    drawn(&app);
    app.settle();
    let prompt = app.reader().password_prompt.expect("its password is asked for, as PDFium needs one");
    app.core().password_answered(prompt.id, Some("right".into()));
    app.settle();
    drawn(&app);
    assert!(app.reader().password_prompt.is_none(), "the password opened it");
    assert_eq!(app.core().engine.note(), None, "its own tab draws with the password");
    opened_by_the_person(&app, &plain, None);
    let added = app.recto(recto_agent::Level::Full, &["page", "insert", crafted.to_str().unwrap(), "--at", "1"]);
    assert!(added.is_ok(), "the loader reads it as plain, so it's added: {added:?}");
    // The view keeps the reader's place, now page 2: the added page is brought into view, where a sleep file would picture it.
    full(&app, &["goto", "1"]);
    drawn(&app);
    assert!(
        app.core().engine.note().is_some_and(|note| note.contains("needs a password")),
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
