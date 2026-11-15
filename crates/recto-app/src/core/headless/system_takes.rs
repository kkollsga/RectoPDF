//! **What the system takes back, and when, on the app's own core:** in the kit the kernel empties nothing the page view keeps unless a test says, so a test
//! coming back to a page or writing a sleep file isn't at the mercy of memory pressure on the machine; and with that held, what a flush writes when nothing
//! has changed.

use super::sleeping::window_view;
use crate::testkit::headless::{Disk, host_turns};

/// Failure (2026-11-14, five tests at load 15 to 77): the kit's page views told the kernel it may empty what they keep off the layers, which it does under
/// memory pressure at moments no test chooses, so a test coming back to a page, or writing a sleep file, found its picture gone one run in twenty. In the
/// kit the system takes only when a test says (`qa memory-pressure discard`, [`recto_reader::Engine::empty_discardable`]): a tab left keeps what it drew
/// flagged discardable and the kernel not told, and a test's own take still empties it.
#[test]
fn in_the_kit_the_system_takes_only_when_a_test_says() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 3), disk.pdf("b.pdf", 3));
    let app = disk.start();
    let engine = app.core().engine.clone();
    for doc in [&a, &b] {
        app.open(doc);
        let mut view = window_view();
        crate::testkit::headless::applied_as_the_host(&engine.set_viewport(view));
        host_turns(&app, &mut view, true);
    }
    assert_eq!(
        engine.qa().marked_for_the_system(),
        0,
        "the kernel may empty what a's left with whenever it likes"
    );
    assert!(engine.empty_discardable() > 0, "a's tiles, kept and discardable, emptied when the test says");
    drop(app.quit());
}

/// The sleep files in the app's state folder, each with its bytes and when it was last written.
fn sleep_files_of(app: &crate::testkit::headless::HeadlessApp) -> Vec<(std::path::PathBuf, Vec<u8>, std::time::SystemTime)> {
    let folder = app.core().saver.store.main_folder().join("journals");
    let mut files: Vec<_> = std::fs::read_dir(&folder)
        .expect("the journals")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("sleep-")))
        .map(|path| {
            let modified = std::fs::metadata(&path).and_then(|meta| meta.modified()).expect("its time");
            (path.clone(), std::fs::read(&path).expect("its bytes"), modified)
        })
        .collect();
    files.sort();
    files
}

/// The app started on `disk`, its tab on screen drawn in a window as the host has it.
fn started_and_drawn(disk: Disk) -> crate::testkit::headless::HeadlessApp {
    let app = disk.start();
    let mut view = window_view();
    crate::testkit::headless::applied_as_the_host(&app.core().engine.set_viewport(view));
    host_turns(&app, &mut view, true);
    app
}

/// Failure (decision 24: "the file's not rewritten when nothing's changed"): a quit rewrites the tab on screen's sleep file with nothing changed, a write
/// to the drive at every quit for nothing. The real app's `a_launch_goes_up_from_the_sleep_file` held this until the kernel, emptying a preview under
/// memory pressure (load 77, 2026-11-14), made its picture come from the tiles and the file change honestly; here the system takes nothing unless a test
/// says. A launch woken from the tab's sleep file, drawn, and quit leaves the file's bytes and time as they were.
#[test]
fn a_quit_with_nothing_changed_writes_nothing() {
    let disk = Disk::new();
    let doc = disk.pdf("a.pdf", 9);
    let app = started_and_drawn(disk);
    app.open(&doc);
    let mut view = window_view();
    host_turns(&app, &mut view, true);
    let (_, disk) = app.quit();
    let app = started_and_drawn(disk);
    let written = sleep_files_of(&app);
    assert_eq!(written.len(), 1, "the quit wrote the tab on screen's");
    assert_eq!(app.core().engine.report().0["last_wake"]["had"], "facts", "woken from it");
    let (_, disk) = app.quit();
    let app = disk.start();
    assert_eq!(sleep_files_of(&app), written, "nothing changed, and a quit wrote its file again");
    drop(app.quit());
}
