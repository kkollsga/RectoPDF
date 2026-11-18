//! The print stays one job through preparing, ready, host handoff and final publication.

use super::*;
use crate::core::headless::printing::unnamed_file;
use crate::testkit::headless::WAIT_LIMIT;
use std::sync::mpsc;

/// Failure: a completion naming no held file consumes and publishes the active QA target even though it owns none of that print.
#[test]
fn an_unowned_completion_cannot_publish_the_active_print() {
    let dir = recto_testpdf::TempDir::new("print-unowned-finish-");
    let printing = Printing::default();
    let out = dir.join("first.pdf");
    let written = printing.qa_target_for(out.clone()).expect("first target");
    std::fs::write(&written, b"first result").unwrap();
    *lock(&printing.ready) = Some((unnamed_file(b"active host file"), 1));
    let (path, _) = printing.take_ready().expect("active host file");
    let before = lock(&printing.qa_target).clone();
    printing.finished("/not-a-print-descriptor");
    let unchanged = *lock(&printing.qa_target) == before;
    let unpublished = !out.exists();
    let bytes = std::fs::read(&path).expect("active descriptor unchanged");
    printing.finished(path.to_str().unwrap());
    assert!(unchanged && unpublished, "only a completion owning the held file may publish its target");
    assert_eq!(bytes, b"active host file");
    assert_eq!(std::fs::read(out).unwrap(), b"first result");
    assert!(!printing.busy());
}

/// Failure: a second panel answer bypasses the disabled Print menu after rendering ends, evicts the first host's descriptor or shares its completion state.
#[test]
fn a_second_print_is_refused_until_the_host_finishes() {
    let printing = Printing::default();
    *lock(&printing.ready) = Some((unnamed_file(b"first print"), 1));
    let accepted_ready = printing.ask(1, 2);
    let (path, serial) = printing.take_ready().expect("the first print");
    let accepted_handing = printing.ask(1, 3);
    let bytes = std::fs::read(&path).expect("the first descriptor remains readable");
    printing.finished(path.to_str().unwrap());
    assert_eq!((accepted_ready, accepted_handing), (false, false), "no new answer while ready or handed off");
    assert_eq!((serial, bytes), (1, b"first print".to_vec()));
    assert!(!printing.busy(), "completion ended the first print");
    assert!(printing.ask(1, 4), "the next print may start after completion");
}

/// Failure: QA changes a busy print's single target before its second action is refused, so the first completion publishes into the second request's path.
#[test]
fn a_busy_prints_qa_target_cannot_be_replaced() {
    let dir = recto_testpdf::TempDir::new("print-target-held-");
    let mut observed = Vec::new();
    for phase in ["preparing", "ready", "handing"] {
        let printing = Printing::default();
        let out = dir.join(format!("{phase}.pdf"));
        let written = printing.qa_target_for(out.clone()).expect("first target");
        let before = lock(&printing.qa_target).clone();
        let handed = match phase {
            "preparing" => {
                assert!(printing.ask(1, 1));
                None
            }
            _ => {
                *lock(&printing.ready) = Some((unnamed_file(b"prepared"), 1));
                (phase == "handing").then(|| printing.take_ready().unwrap().0)
            }
        };
        let replacement = printing.qa_target_for(dir.join(format!("other-{phase}.pdf")));
        observed.push((phase, replacement.is_none(), *lock(&printing.qa_target) == before));
        if let Some(path) = handed {
            std::fs::write(&written, b"the first result").unwrap();
            printing.finished(path.to_str().unwrap());
        }
    }
    assert!(observed.iter().all(|(_, refused, unchanged)| *refused && *unchanged), "{observed:?}");
    assert_eq!(std::fs::read(dir.join("handing.pdf")).unwrap(), b"the first result");
    assert!(!dir.join("other-handing.pdf").exists(), "the refused target was never used");
}

/// Failure: taking ready empties that slot before the host's slot is occupied, letting another observer see idle in the middle of a handoff.
#[test]
fn ready_to_handing_has_no_observable_idle_gap() {
    let printing = Arc::new(Printing::default());
    *lock(&printing.ready) = Some((unnamed_file(b"handoff"), 1));
    let (entered, held) = mpsc::channel();
    let (release, waiting) = mpsc::channel();
    *lock(&AFTER_READY_TAKE) = Some((entered, waiting));
    let worker = Arc::clone(&printing);
    let taking = std::thread::Builder::new()
        .name("print-handoff-test".into())
        .spawn(move || worker.take_ready())
        .unwrap();
    let reached = held.recv_timeout(WAIT_LIMIT);
    let protected = match printing.ready.try_lock() {
        Err(std::sync::TryLockError::WouldBlock) => true, // busy must wait until the ownership transition is complete.
        Ok(ready) => printing.running.load(Ordering::SeqCst) || ready.is_some() || !lock(&printing.handing).is_empty(),
        Err(std::sync::TryLockError::Poisoned(_)) => false,
    };
    drop(release);
    let result = taking.join().expect("handoff ended").expect("host file");
    let still_busy = printing.busy();
    let bytes = std::fs::read(&result.0).expect("host descriptor");
    printing.finished(result.0.to_str().unwrap());
    reached.expect("handoff reached the held transition");
    assert!(protected, "the ready slot and host slot were both observably empty");
    assert!(still_busy);
    assert_eq!(bytes, b"handoff");
}

/// Failure: the print goes idle or loses its descriptor before final output publication, so a new request can retarget it or an update can leave it unfinished.
#[test]
fn a_print_stays_busy_and_owns_its_file_through_publication() {
    let dir = recto_testpdf::TempDir::new("print-publication-held-");
    let printing = Arc::new(Printing::default());
    let out = dir.join("first.pdf");
    let written = printing.qa_target_for(out.clone()).expect("first target");
    std::fs::write(&written, b"published first").unwrap();
    *lock(&printing.ready) = Some((unnamed_file(b"host file"), 1));
    let (path, _) = printing.take_ready().expect("host file");
    let (entered, held) = mpsc::channel();
    let (release, waiting) = mpsc::channel();
    *lock(&BEFORE_PUBLICATION) = Some((entered, waiting));
    let worker = Arc::clone(&printing);
    let finishing_path = path.clone();
    let finishing = std::thread::Builder::new()
        .name("print-finish-test".into())
        .spawn(move || worker.finished(finishing_path.to_str().unwrap()))
        .unwrap();
    let reached = held.recv_timeout(WAIT_LIMIT);
    let busy = printing.busy();
    let readable = std::fs::read(&path).ok();
    let accepted = printing.ask(1, 2);
    let retargeted = printing.qa_target_for(dir.join("other.pdf"));
    drop(release);
    finishing.join().expect("publication ended");
    reached.expect("publication reached the hold");
    assert!(busy, "publication is still part of this print");
    assert_eq!(readable.as_deref(), Some(b"host file".as_slice()), "host descriptor still held");
    assert!(!accepted, "no second print during publication");
    assert!(retargeted.is_none(), "no replacement QA target during publication");
    assert_eq!(std::fs::read(out).unwrap(), b"published first");
    assert!(!printing.busy());
    assert!(printing.qa_target_for(dir.join("next.pdf")).is_some());
    assert!(printing.ask(1, 3), "a new print works once publication finishes");
}
