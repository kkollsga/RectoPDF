//! The provenance ledger as the app writes it (`docs/design/removal.md` §10, D2b): the person's own Save and the record write each leave their entries in
//! the state folder's `removal/ledger.v1`, through `AppSaving` and the record store.

use crate::testkit::headless::{Disk, HeadlessApp};
use recto_core::removal::{Ledger, Link, read_ledger};

fn ledger(app: &HeadlessApp) -> Ledger {
    recto_control::removal_store::ledger::read(&app.disk().paths().root)
}

/// Failure: the app's `AppSaving` doesn't forward a save's steps to the ledger (an empty `provenance`), or the record write doesn't note its tabs, so a
/// backup this app made is nobody's to the removal and every removal refuses. The tab's file is noted when the record is written, and the person's Save,
/// from the window's own lane, leaves the file's version before and after it, joined by a `Saved`, with no gap.
#[test]
fn the_record_and_the_persons_save_write_the_ledger() {
    let disk = Disk::new();
    let file = disk.pdf("doc.pdf", 3);
    let app = disk.builder().start();
    app.open(&file);
    app.settle();
    // The record is written a moment after the tab opens, on the clock, and at once when asked.
    app.core().saver.write_soon();
    app.advance(6_000);
    app.settle();
    let canonical = file.canonicalize().expect("the file's real path");
    app.wait_for("the tab's file in the ledger", || {
        let found = ledger(&app);
        let there = found.links.iter().any(|link| matches!(link, Link::At { path, .. } if *path == canonical));
        there
            .then_some(())
            .ok_or_else(|| format!("no `At` yet: {:?}; record {}", found.links, app.record_json()))
    });
    let first = ledger(&app);
    assert_eq!(first.gap, None, "{:?}", first.links);
    app.core().perform("rotate.right".into());
    app.settle();
    app.core().perform("file.save".into());
    app.settle();
    app.wait_for("the save's own entries", || {
        let found = ledger(&app);
        let there = found.links.iter().any(|link| matches!(link, Link::Saved { .. }));
        there.then_some(()).ok_or_else(|| format!("no `Saved` yet: {:?}", found.links))
    });
    let saved = ledger(&app);
    assert_eq!(saved.gap, None, "the backup went where its path hashes to: {:?}", saved.links);
    let bytes = std::fs::read(app.disk().paths().root.join("removal").join("ledger.v1")).expect("the ledger");
    assert_eq!(
        read_ledger(&bytes, &recto_core::removal::LEDGER_LIMITS).links,
        saved.links,
        "what is read is what is on disk"
    );
}
