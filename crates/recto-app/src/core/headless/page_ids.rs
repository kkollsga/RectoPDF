//! **The page-identity thread on the app's own core** (`core/page_ids.rs`, the page matcher's first part): an edit's pages are read for what they are,
//! off the session's lock, and go in the journal's sidecar, never the journal; never for a document opened with its password; read again after a
//! restart without waiting for an edit; a list closed by its budget can't fail a hand-off or a dormancy; and the thread is gone once there's nothing to read.

use super::withheld_events::{person_unlocks, protected};
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::Level;
use recto_session::TabId;
use recto_session::edit::identity;
use std::path::PathBuf;

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

fn journals(app: &HeadlessApp) -> PathBuf {
    app.core().saver.store.main_folder().join("journals")
}

/// The tab's journal's name, once it has one.
fn journal_name(app: &HeadlessApp, id: TabId) -> Option<String> {
    app.core()
        .host
        .session
        .with_session(|session| session.journals().name_of(id).map(str::to_owned))
}

/// The tab's journal's sidecar, as read, once it's there.
fn sidecar(app: &HeadlessApp, id: TabId) -> Option<identity::sidecar::Sidecar> {
    let name = journal_name(app, id)?;
    let key = name.strip_prefix("journal-")?.strip_suffix(".log")?;
    identity::sidecar::decode(&std::fs::read(journals(app).join(format!("pages-{key}.ids"))).ok()?)
}

/// What QA says of the reading: named, read, reading, caught up.
fn pages_report(app: &HeadlessApp) -> serde_json::Value {
    app.record_json()["pages"].clone()
}

/// Waits for every page named to be read and the thread gone, then for the journals and sidecars to have it all (they go a moment after the last
/// change, which the clock is moved past).
fn read_and_written(app: &HeadlessApp, what: &str) {
    app.wait_until(what, || {
        let report = pages_report(app);
        report["named"].as_u64().is_some_and(|named| named > 0) && report["read"] == report["named"] && report["reading"] == false
    });
    app.advance(6_000);
    app.wait_until(what, || pages_report(app)["caught_up"] == true);
}

/// Failure caught: an edit's pages are never read for what they are (no thread starts, or it reads and drops what it read), so the edits can't follow
/// their pages when the file changes; what's read never reaches the sidecar, so a restart loses it; it reaches the journal (B1: then an app from before
/// can't read it); or the thread stays once there's nothing to read (rule 1). A turn of page 9 of ten: pages 7 to 10 are read and in the sidecar, the
/// journal is the version an app from before reads, with no line of them, and the thread is gone.
#[test]
fn an_edits_pages_are_read_into_the_sidecar_and_the_thread_goes() {
    let app = HeadlessApp::start();
    let file = app.disk().pdf("doc.pdf", 10);
    app.open(&file);
    app.recto(Level::Control, &["page", "rotate", "9", "--by", "90"]).expect("rotated");
    read_and_written(&app, "the edited pages read and written");
    let (id, pages) = named(&app);
    assert_eq!(pages.iter().map(|(page, _)| *page).collect::<Vec<_>>(), vec![6, 7, 8, 9]);
    let read = sidecar(&app, id).expect("a sidecar");
    assert_eq!(read.pages.iter().map(|(page, _)| *page).collect::<Vec<_>>(), vec![6, 7, 8, 9]);
    let journal = std::fs::read(journals(&app).join(journal_name(&app, id).unwrap())).unwrap();
    let parsed = recto_session::record::parse_journal(&journal, recto_session::record::CURRENT_VERSION).expect("an app from before reads it");
    assert_eq!(parsed.ops.len(), 1, "the turn alone");
    app.settle();
    assert!(!app.core().tools.page_ids().busy(), "no thread once there's nothing to read");
}

/// Failure caught (the coordinator's rule, 2026-11-15): a document the person opened with its password has its pages read for their digests, which go
/// on disk, though nothing may be derived from such a document's content without the person's password. Its edit's pages are named and never read, and
/// it has no sidecar. Two things hold it, and this sees the end of both: the session never asks for such a tab (`recto-session`'s
/// `a_protected_tabs_pages_are_never_read`), and the core's open refuses a file needing a password it isn't given.
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
    app.wait_until("the edit in its journal", || {
        pages_report(&app)["caught_up"] == true && journal_name(&app, id).is_some()
    });
    let names: Vec<String> = std::fs::read_dir(journals(&app))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|name| !name.starts_with("pages-")), "no sidecar: {names:?}");
}

/// Ten pages that each cost a ninth of a version's budget, or a little more (`recto_session::edit::identity::VERSION_BYTES`): each page's `/Contents`
/// is 255 references to one stream of 1 MiB that decodes to nothing (an empty zlib stream, then spaces), charged its stored length each time, as a
/// crafted file would be. A file of about 1 MiB whose reading gives up after nine pages.
fn ten_past_the_budget(disk: &Disk) -> PathBuf {
    use recto_testpdf::Object;
    let kids: String = (0..10).map(|at| format!("{} 0 R ", 3 + at)).collect();
    let mut objects = vec![
        Object::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
        Object::Dict(format!("<< /Type /Pages /Kids [{kids}] /Count 10 >>")),
    ];
    let many: String = (0..255).map(|_| "13 0 R ").collect();
    objects.extend((0..10).map(|_| Object::Dict(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents [{many}] >>"))));
    let mut nothing = vec![0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01];
    nothing.resize(1 << 20, b' ');
    objects.push(Object::Stream {
        dict: "/Filter /FlateDecode".into(),
        data: nothing,
    });
    disk.file("costly.pdf", &recto_testpdf::build(&objects))
}

/// Failure caught (second review): a list closed by its version's budget (`identity::give_up`) leaves what it read waiting for the sidecar with
/// nothing to write it from, so the journals are never caught up and the tab can never checkpoint or go dormant, nor a hand-off pass (seen: with
/// `give_up` dropping the pages read with those not read, this test waits out its limit for `caught_up`). A turn of every page of a file whose pages
/// each cost a ninth of the budget: nine are read and in the sidecar, the list is closed, the tab goes dormant and a hand-off's read-back passes.
#[test]
fn a_list_closed_by_its_budget_still_sleeps_and_hands_off() {
    let disk = Disk::new();
    let file = ten_past_the_budget(&disk);
    let other = disk.pdf("other.pdf", 2);
    let app = disk.start();
    app.open(&file);
    app.recto(Level::Control, &["page", "rotate", "1-10", "--by", "90"]).expect("rotated");
    read_and_written(&app, "reading given up and written");
    let (id, pages) = named(&app);
    assert_eq!(
        pages.iter().map(|(page, _)| *page).collect::<Vec<_>>(),
        (0..9).collect::<Vec<u32>>(),
        "read to the budget"
    );
    assert_eq!(sidecar(&app, id).expect("a sidecar").pages.len(), 9);
    app.core().host.session.with_session(|session| {
        assert!(identity::identities(session.lists().get(id).unwrap()).unwrap().over_cap());
    });
    app.open(&other);
    let slept = app.recto(Level::Full, &["session", "sleep", "--tab", &format!("#{}", id.0)]).expect("slept");
    assert_eq!(slept["dormant"], true, "{slept}");
    app.core()
        .hand_off(crate::testkit::headless::WAIT_LIMIT)
        .expect("a hand-off's read-back passes");
}

/// Failure caught (review: "a look after restore"): a list restored from its journal with pages named and not read (a quit while they were being read)
/// waits for the next edit to have them read, so if the file changes first its edits can't follow. A journal of a turn left by a crash, with no sidecar:
/// at the restart its pages are read without an edit, and go in the sidecar.
#[test]
fn a_restored_lists_unread_pages_are_read_without_waiting_for_an_edit() {
    let disk = Disk::new();
    let file = disk.pdf("doc.pdf", 10);
    let stamp = recto_platform::file_stamp(&file).unwrap();
    let folder = disk.paths().root.join("session").join("journals");
    // Private to this user, as the state folder's own are, or the store won't write there.
    #[cfg(unix)]
    let made = std::os::unix::fs::DirBuilderExt::mode(std::fs::DirBuilder::new().recursive(true), 0o700).create(&folder);
    #[cfg(not(unix))]
    let made = std::fs::create_dir_all(&folder);
    made.unwrap();
    let header = recto_session::record::JournalHeader::new(1, &file, Some(stamp), 10, 0);
    let mut journal = recto_session::record::journal_start(&header, None);
    journal.extend_from_slice(&recto_session::record::journal_lines(&[recto_session::record::JournalOp::Rotate(
        4,
        1,
        1,
        recto_session::record::AuthorName::Owner,
    )]));
    let name = recto_session::record::journal_file_name(Some(stamp), &file, None);
    std::fs::write(folder.join(&name), journal).unwrap();
    let app = disk.start();
    read_and_written(&app, "the restored list's pages read and written");
    let (id, pages) = named(&app);
    assert_eq!(pages.iter().map(|(page, _)| *page).collect::<Vec<_>>(), vec![2, 3, 4, 5, 6]);
    assert_eq!(sidecar(&app, id).expect("a sidecar").pages.len(), 5);
}

/// **Failure caught: a page's digest reaches an agent, the person's window, a log, or rests anywhere but its sidecar** (the coordinator's rule,
/// 2026-11-15: a digest is of a document's content; `qa state` printed every named page's, in hex, until slice 1's review of its own full check). A tab
/// whose pages are named and read, by the person's edit and an agent's at Full: no digest, in hex or as its bytes, is in any answer an agent gets at
/// Full, `qa state`, QA's record report, any event, what the window is given, the Debug print of the list, its record or what was read, or any file of
/// the state folder but the journal's sidecar, which does hold them (so the search is seen to find them where they are).
#[test]
fn no_digest_reaches_an_answer_an_event_the_window_or_a_log() {
    let app = HeadlessApp::start();
    let file = app.disk().pdf("doc.pdf", 10);
    app.open(&file);
    app.hear_events();
    app.person(r#"{"cmd":"page_rotate","tab":"active","pages":"3","degrees":90}"#);
    app.recto(Level::Full, &["page", "rotate", "8", "--by", "90"]).expect("rotated");
    read_and_written(&app, "the edited pages read and written");
    let (id, _) = named(&app);
    let (digests, debug) = app.core().host.session.with_session(|session| {
        let edits = session.lists().get(id).expect("the list");
        let digests: Vec<[u8; 16]> = identity::identities(edits)
            .expect("named")
            .pages()
            .iter()
            .filter_map(|(_, known)| known.map(|known| known.digest))
            .collect();
        let read: Vec<_> = identity::identities(edits).unwrap().pages().to_vec();
        (
            digests,
            format!("{edits:?} {:?} {read:?} {:?}", edits.to_record(), identity::sidecar_waiting(edits)),
        )
    });
    assert!(digests.len() >= 6, "{}", digests.len());
    let hex: Vec<String> = digests.iter().map(|digest| digest.iter().map(|byte| format!("{byte:02x}")).collect()).collect();
    let mut seen: Vec<(String, Vec<u8>)> = vec![("Debug of the list, its record and what was read".to_string(), debug.into_bytes())];
    for words in [&["tabs"][..], &["where"], &["edits"], &["session", "show"], &["pages"], &["undo"], &["redo"]] {
        let answer = app
            .recto(Level::Full, words)
            .map_or_else(|error| format!("{error:?}"), |answer| answer.to_string());
        seen.push((format!("recto {}", words.join(" ")), answer.into_bytes()));
    }
    // `qa state`'s session and record are these two, which the real app's QA bridge answers with (a headless core has no bridge to ask through).
    seen.push(("qa state's session".into(), app.session_json().to_string().into_bytes()));
    seen.push(("qa state's record".into(), app.record_json().to_string().into_bytes()));
    seen.push(("events".into(), format!("{:?}", app.heard_events()).into_bytes()));
    seen.push(("what the host heard".into(), format!("{:?}", app.heard()).into_bytes()));
    seen.push(("the window's reader".into(), format!("{:?}", app.reader()).into_bytes()));
    seen.push(("the sidebar".into(), format!("{:?}", app.sidebar()).into_bytes()));
    let mut sidecars = 0;
    let mut folders = vec![app.disk().paths().root.clone()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                let is_sidecar = path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("pages-"));
                if is_sidecar {
                    sidecars += 1;
                    assert!(
                        digests.iter().all(|digest| bytes.windows(16).any(|window| window == digest)),
                        "the sidecar holds them"
                    );
                } else {
                    seen.push((path.display().to_string(), bytes));
                }
            }
        }
    }
    assert_eq!(sidecars, 1, "one sidecar, the tab's");
    for (what, bytes) in &seen {
        for (digest, hex) in digests.iter().zip(&hex) {
            assert!(!bytes.windows(16).any(|window| window == digest), "a digest's bytes in {what}");
            assert!(!bytes.windows(hex.len()).any(|window| window == hex.as_bytes()), "a digest ({hex}) in {what}");
        }
    }
}
