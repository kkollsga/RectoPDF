use super::*;
use crate::core::headless::printing::unnamed_file;
use crate::core::printing::fixtures;
use crate::core::running::Running;
use crate::testkit::headless::{HeadlessApp, WAIT_LIMIT};

/// Failure: a refused worker start loses the file, target or way-back, spins retries, or never retries. The injected clock crosses exactly the one-second boundary.
#[test]
fn a_failed_publication_start_retains_work_until_its_retry_is_due() {
    let dir = recto_testpdf::TempDir::new("print-retry-");
    let app = HeadlessApp::start();
    let printing = app.core().tools.printing();
    let out = dir.join("out.pdf");
    let written = printing.qa_target_for(out.clone()).unwrap();
    std::fs::write(&written, b"published after retry").unwrap();
    let ready = fixtures::handed(printing, unnamed_file(b"still owned"), 4);
    fixtures::refuse_spawns(printing, 1);
    request(printing, &app.core().core, &ready.path, ready.serial);
    app.wait_until("the failed start is deferred", || {
        printing.completion_due(app.core().life.now_ms()) == Some(Duration::from_millis(RETRY_MS))
    });
    assert_eq!(fixtures::worker_starts(printing), 0);
    assert!(printing.publishing() && printing.busy());
    assert_eq!(app.core().core.ways_back_held(), 1, "pending work still holds settle");
    assert_eq!(std::fs::read(&ready.path).unwrap(), b"still owned");
    assert!(written.exists() && !out.exists());
    app.clock().advance(RETRY_MS - 1);
    app.core().core.nudge();
    assert!(app.core().core.flush(WAIT_LIMIT));
    assert_eq!(fixtures::worker_starts(printing), 0);
    assert_eq!(printing.completion_due(app.core().life.now_ms()), Some(Duration::from_millis(1)));
    app.clock().advance(1);
    app.core().core.nudge();
    app.settle();
    assert_eq!(fixtures::worker_starts(printing), 1);
    assert_eq!(std::fs::read(out).unwrap(), b"published after retry");
    assert!(!written.exists() && !printing.busy());
    assert_eq!(app.core().core.ways_back_held(), 0);
}

/// Failure: quit waits on the whole printer lifetime, or fails to include publication in its existing bounded grace; handoff and settle also retain the owned job.
#[test]
fn only_publication_is_counted_in_the_quit_grace() {
    let dir = recto_testpdf::TempDir::new("print-quit-phase-");
    let app = HeadlessApp::start();
    let inner = app.core();
    let printing = inner.tools.printing();
    let written = printing.qa_target_for(dir.join("out.pdf")).unwrap();
    std::fs::write(written, b"finished").unwrap();
    let ready = fixtures::handed(printing, unnamed_file(b"host"), 5);
    assert!(
        !Running::now(&inner.life, &inner.host, &inner.tools).changes_what_a_quit_writes(),
        "a printer can wait without holding quit"
    );
    let (held, release) = fixtures::hold_publication(printing);
    request(printing, &inner.core, &ready.path, ready.serial);
    let reached = held.recv_timeout(WAIT_LIMIT);
    let counted = Running::now(&inner.life, &inner.host, &inner.tools).changes_what_a_quit_writes();
    let unsafe_to_hand_off = !inner.hand_off_check().safe;
    let way_back = inner.core.ways_back_held();
    drop(release);
    app.settle();
    reached.expect("actual publication was held");
    assert!(counted && unsafe_to_hand_off);
    assert_eq!(way_back, 1);
    assert!(!Running::now(&inner.life, &inner.host, &inner.tools).changes_what_a_quit_writes());
    assert_eq!(inner.core.ways_back_held(), 0);
}

/// Failure: the cap evicts an outstanding host descriptor (possibly before background open), or leaves ready visible at a full cap so the host spins taking None.
#[test]
fn a_full_handoff_cap_keeps_every_file_and_withholds_the_next_offer() {
    let printing = Printing::default();
    let mut keys = Vec::new();
    for serial in 0..super::super::MAX_HANDING as u64 {
        keys.push(fixtures::handed(&printing, unnamed_file(b"held"), serial));
    }
    *lock(&printing.ready) = Some((unnamed_file(b"waiting"), 99));
    let offered_at_cap = printing.has_ready();
    let overflow = printing.take_ready();
    let readable: Vec<_> = keys.iter().map(|key| std::fs::read(&key.path).ok()).collect();
    printing.finished(&keys.remove(0).path);
    let offered_with_room = printing.has_ready();
    let next = printing.take_ready();
    for key in keys {
        printing.finished(&key.path);
    }
    if let Some((path, _)) = &next {
        printing.finished(path.to_str().unwrap());
    }
    assert!(!offered_at_cap && overflow.is_none(), "no repeated unusable offer and no cap eviction");
    assert!(readable.iter().all(|bytes| bytes.as_deref() == Some(b"held".as_slice())));
    assert!(offered_with_room);
    assert_eq!(next.map(|(_, serial)| serial), Some(99));
    assert!(!printing.busy());
}
