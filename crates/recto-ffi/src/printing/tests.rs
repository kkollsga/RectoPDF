use super::*;
use recto_app::core::printing::fixtures;
use recto_app::testkit::headless::{HeadlessApp, WAIT_LIMIT};
use std::io::Write;
use std::sync::{Arc, mpsc};

fn file(dir: &std::path::Path) -> std::fs::File {
    use std::os::unix::fs::OpenOptionsExt;
    let path = dir.join("prepared");
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    std::fs::remove_file(path).unwrap();
    file.write_all(b"prepared pages").unwrap();
    file
}

/// Failure: the actual exported completion waits for publication on its caller, even though the host must return to its UI immediately.
#[test]
fn print_finished_returns_while_real_publication_is_held() {
    let dir = recto_testpdf::TempDir::new("ffi-print-publish-");
    let app = HeadlessApp::start();
    let core = Arc::new(AppCore {
        inner: Arc::clone(app.core()),
        qa_mode: true,
        start_with: Vec::new(),
    });
    let printing = app.core().tools.printing();
    let out = dir.join("out.pdf");
    let written = printing.qa_target_for(out.clone()).unwrap();
    std::fs::write(&written, b"complete PDF").unwrap();
    let ready = fixtures::handed(printing, file(dir.path()), 7);
    let (held, release) = fixtures::hold_publication(printing);
    let (returned, answer) = mpsc::channel();
    let caller = Arc::clone(&core);
    let path = ready.path.clone();
    let thread = std::thread::Builder::new()
        .name("print-ffi-caller".into())
        .spawn(move || {
            caller.print_finished(path, 7);
            let _ = returned.send(());
        })
        .unwrap();
    let reached = held.recv_timeout(WAIT_LIMIT);
    let replied = answer.recv_timeout(WAIT_LIMIT);
    let busy = printing.busy();
    let bytes = std::fs::read(&ready.path).ok();
    core.print_finished(ready.path.clone(), ready.serial);
    core.print_finished(ready.path.clone(), ready.serial + 1);
    core.print_finished("/not-an-owned-print".into(), ready.serial);
    let starts = fixtures::worker_starts(printing);
    drop(release);
    thread.join().unwrap();
    app.settle();
    reached.expect("the real publication boundary was held");
    replied.expect("FFI must return while publication is still held");
    assert!(busy);
    assert_eq!(starts, 1, "duplicates and unowned keys start no worker");
    assert_eq!(bytes.as_deref(), Some(b"prepared pages".as_slice()));
    assert_eq!(std::fs::read(out).unwrap(), b"complete PDF");
    assert!(!written.exists());
    assert!(!printing.busy());
}

/// Failure: an output appearing after the QA target was chosen is overwritten by the asynchronous completion, or its staging file and descriptor leak on refusal.
#[test]
fn print_finished_preserves_an_output_that_appeared_meanwhile() {
    let dir = recto_testpdf::TempDir::new("ffi-print-raced-output-");
    let app = HeadlessApp::start();
    let core = AppCore {
        inner: Arc::clone(app.core()),
        qa_mode: true,
        start_with: Vec::new(),
    };
    let printing = app.core().tools.printing();
    let out = dir.join("out.pdf");
    let written = printing.qa_target_for(out.clone()).unwrap();
    std::fs::write(&written, b"the prepared output").unwrap();
    let ready = fixtures::handed(printing, file(dir.path()), 11);
    std::fs::write(&out, b"sentinel").unwrap();
    core.print_finished(ready.path.clone(), ready.serial);
    app.settle();
    assert_eq!(std::fs::read(out).unwrap(), b"sentinel");
    assert!(!written.exists());
    assert!(std::fs::metadata(ready.path).is_err());
    assert!(!printing.busy());
}

/// Failure: an old completion with a reused numeric descriptor can finish a different serial's current print.
#[test]
fn print_finished_requires_the_held_serial() {
    let dir = recto_testpdf::TempDir::new("ffi-print-key-");
    let app = HeadlessApp::start();
    let core = AppCore {
        inner: Arc::clone(app.core()),
        qa_mode: true,
        start_with: Vec::new(),
    };
    let printing = app.core().tools.printing();
    let ready = fixtures::handed(printing, file(dir.path()), 9);
    core.print_finished(ready.path.clone(), 8);
    let preserved = std::fs::read(&ready.path).ok();
    core.print_finished(ready.path, 9);
    app.settle();
    assert_eq!(preserved.as_deref(), Some(b"prepared pages".as_slice()), "wrong serial must do nothing");
}
