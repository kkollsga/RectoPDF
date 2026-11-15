//! **The page-identity thread on the app's own core** (`core/page_ids.rs`, the page matcher's first part): an edit's pages are read for what they are,
//! off the session's lock, and go in the journal; never for a document opened with its password; and the thread is gone once there's nothing to read.

use super::withheld_events::{person_unlocks, protected};
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::Level;
use recto_session::TabId;
use recto_session::edit::identity;

/// The one tab's id, and its named pages: each with whether it's been read.
fn named(app: &HeadlessApp) -> (TabId, Vec<(u32, bool)>) {
    app.core().host.session.with_session(|session| {
        let id = session.tabs().iter().next().expect("a tab").id;
        let pages = session
            .lists()
            .get(id)
            .and_then(identity::identities)
            .map(|ids| ids.pages().iter().map(|(page, known)| (*page, known.is_some())).collect())
            .unwrap_or_default();
        (id, pages)
    })
}

/// The tab's journal on disk, as this app reads it, once it's there.
fn journal(app: &HeadlessApp, id: TabId) -> Option<recto_session::record::ParsedJournal> {
    let name = app
        .core()
        .host
        .session
        .with_session(|session| session.journals().name_of(id).map(str::to_owned))?;
    let bytes = std::fs::read(app.core().saver.store.main_folder().join("journals").join(name)).ok()?;
    Some(recto_session::record::parse_journal(&bytes, recto_session::record::JOURNAL_VERSION).expect("it reads"))
}

fn has_identified(journal: Option<recto_session::record::ParsedJournal>) -> bool {
    journal.is_some_and(|journal| journal.ops.iter().any(|op| matches!(op, recto_session::record::JournalOp::Identified(_))))
}

/// Failure caught: an edit's pages are never read for what they are (no thread starts, or it reads and drops what it read), so the edits can't follow
/// their pages when the file changes; what's read never reaches the journal, so a restart loses it; or the thread stays once there's nothing to read
/// (rule 1). A turn of page 9 of ten: pages 7 to 10 are read and in the journal, which says the newer version, and the thread is gone.
#[test]
fn an_edits_pages_are_read_and_journaled_and_the_thread_goes() {
    let app = HeadlessApp::start();
    let file = app.disk().pdf("doc.pdf", 10);
    app.open(&file);
    app.recto(Level::Control, &["page", "rotate", "9", "--by", "90"]).expect("rotated");
    app.wait_until("the edited pages read", || {
        named(&app).1.iter().all(|(_, read)| *read) && !named(&app).1.is_empty()
    });
    let (id, pages) = named(&app);
    assert_eq!(pages.iter().map(|(page, _)| *page).collect::<Vec<_>>(), vec![6, 7, 8, 9]);
    app.advance(6_000);
    app.wait_until("what was read in the journal", || has_identified(journal(&app, id)));
    assert_eq!(journal(&app, id).unwrap().header.version, recto_session::record::JOURNAL_VERSION);
    app.settle();
    assert!(!app.core().tools.page_ids().busy(), "no thread once there's nothing to read");
}

/// Failure caught (the coordinator's rule, 2026-11-15): a document the person opened with its password has its pages read for their digests, which go in
/// its journal on disk, though nothing may be derived from such a document's content without the person's password. Its edit's pages are named and
/// never read, and its journal has no line of them. Two things hold it, and this sees the end of both: the session never asks for such a tab
/// (`recto-session`'s `a_protected_tabs_pages_are_never_read`, seen failing with that rule taken out, when this still passed), and the core's open
/// refuses a file needing a password it isn't given.
#[test]
fn a_password_opened_documents_pages_are_never_read() {
    let disk = Disk::new();
    let file = protected(&disk, 3, "Locked");
    let app = disk.start();
    person_unlocks(&app, &file);
    app.person(r#"{"cmd":"page_rotate","pages":"2","degrees":90}"#);
    app.settle();
    let (id, pages) = named(&app);
    assert!(!pages.is_empty(), "the edit named its pages");
    assert!(pages.iter().all(|(_, read)| !read), "none read: {pages:?}");
    app.advance(6_000);
    app.settle();
    app.wait_until("the edit in its journal", || journal(&app, id).is_some_and(|journal| !journal.ops.is_empty()));
    assert!(!has_identified(journal(&app, id)), "no line of what its pages are");
}

/// **Failure caught: a page's digest reaches an agent, the person's window, or a log** (the coordinator's rule, 2026-11-15: a digest is of a document's
/// content; `qa state` printed every named page's, in hex, until slice 1's review of its own full check). A tab whose pages are named and read, by the
/// person's edit and an agent's at Full: no digest, in hex, or packed as the journal keeps it, or its first 21 characters (the digest's part of the
/// packed string), is in any answer an agent gets at Full, `qa state`, any event, what the window is given, the Debug print of the list or
/// its record, or any file of the state folder but the journals, which keep them by design.
#[test]
fn no_digest_reaches_an_answer_an_event_the_window_or_a_log() {
    let app = HeadlessApp::start();
    let file = app.disk().pdf("doc.pdf", 10);
    app.open(&file);
    app.hear_events();
    app.person(r#"{"cmd":"page_rotate","tab":"active","pages":"3","degrees":90}"#);
    app.recto(Level::Full, &["page", "rotate", "8", "--by", "90"]).expect("rotated");
    app.wait_until("the edited pages read", || {
        let pages = named(&app).1;
        !pages.is_empty() && pages.iter().all(|(_, read)| *read)
    });
    app.advance(6_000);
    let (id, _) = named(&app);
    app.wait_until("what was read in the journal", || has_identified(journal(&app, id)));
    // What would give a digest away: each page's, in hex, packed, and the packed string's digest part.
    let (secrets, debug) = app.core().host.session.with_session(|session| {
        let edits = session.lists().get(id).expect("the list");
        let mut secrets: Vec<String> = identity::identities(edits)
            .expect("named")
            .pages()
            .iter()
            .filter_map(|(_, known)| known.map(|known| known.digest.iter().map(|byte| format!("{byte:02x}")).collect()))
            .collect();
        for page in edits.to_record().identities.expect("named").pages {
            let packed = page.1.expect("read");
            secrets.push(packed[..21].to_string());
            secrets.push(packed);
        }
        (secrets, format!("{edits:?} {:?}", edits.to_record()))
    });
    assert!(secrets.len() >= 12, "{secrets:?}");
    let mut seen = vec![("Debug of the list and its record".to_string(), debug)];
    for words in [&["tabs"][..], &["where"], &["edits"], &["session", "show"], &["pages"], &["undo"], &["redo"]] {
        let answer = app
            .recto(Level::Full, words)
            .map_or_else(|error| format!("{error:?}"), |answer| answer.to_string());
        seen.push((format!("recto {}", words.join(" ")), answer));
    }

    // `qa state`'s session and record are these two, which the real app's QA bridge answers with (a headless core has no bridge to ask through).
    let session = app.session_json().to_string();
    assert!(session.contains("identities"), "qa state's session says the named pages");
    seen.push(("qa state's session".into(), session));
    seen.push(("qa state's record".into(), app.record_json().to_string()));
    seen.push(("events".into(), format!("{:?}", app.heard_events())));
    seen.push(("what the host heard".into(), format!("{:?}", app.heard())));
    seen.push(("the window's reader".into(), format!("{:?}", app.reader())));
    seen.push(("the sidebar".into(), format!("{:?}", app.sidebar())));
    let state = app.core().saver.store.main_folder();
    let mut folders = vec![state];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_none_or(|name| name != "journals") {
                    folders.push(path);
                }
            } else if let Ok(bytes) = std::fs::read(&path) {
                seen.push((path.display().to_string(), String::from_utf8_lossy(&bytes).into_owned()));
            }
        }
    }
    for (what, text) in &seen {
        for secret in &secrets {
            assert!(!text.contains(secret.as_str()), "a digest ({secret}) in {what}");
        }
    }
}
