//! **A launch that doesn't get the record's lock at its start.** The copy before it is a store the test holds, let go of when the test says.

use crate::testkit::headless::Disk;
use recto_control::record_store::{Flavor, RecordStore};
use recto_session::record::notices;

/// The words of the notice a window that won't be remembered shows.
fn not_kept_words() -> Option<String> {
    notices::words(notices::OTHER_INSTANCE).map(str::to_owned)
}

/// The copy before this one, still holding the record: a store of the test's on the disk.
fn the_copy_before(disk: &Disk) -> RecordStore {
    let store = RecordStore::open(disk.paths(), Flavor::App);
    assert!(store.holds_the_record(), "the copy before keeps the record");
    store
}

/// Failure (found on the way, 2026-11-13): a window that doesn't keep the record tries to write it when something comes due (a minute after a change, or at once
/// when the app goes to the background), is refused, and says "RectoPDF couldn't save its state, so what's changed since the last save would be lost if it
/// closed now" over the notice that it won't be remembered: wrong, and alarming, as nothing it could save is being lost. It writes nothing and says only that.
#[test]
fn a_window_that_doesnt_keep_the_record_never_says_its_state_couldnt_be_saved() {
    let disk = Disk::new();
    let before = the_copy_before(&disk);
    let app = disk.builder().another_keeps().start();
    let c = app.disk().pdf("c.pdf", 2);
    app.open(&c);
    app.core().saver.write_soon();
    app.settle();
    // Past the record's minute, and past any trying.
    app.advance(70_000);
    app.settle();
    assert!(!app.core().saver.has_failed(), "{:?}", app.core().saver.last_error());
    assert_eq!(app.reader().note, not_kept_words());
    drop(before);
}
