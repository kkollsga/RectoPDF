//! Concurrent host requests retain the most recently admitted request still pending.

use super::*;
use crate::testkit::headless::HeadlessApp;
use std::sync::mpsc;

const LIMIT: Duration = Duration::from_secs(5);

struct Requests(mpsc::Sender<(u64, String)>);

impl crate::Listener for Requests {
    fn model_changed(&self) {}
    fn ops_ready(&self) {}
    fn qa_request(&self, id: u64, verb: String, _value: String) {
        let _ = self.0.send((id, verb));
    }
    fn session_ended(&self) {}
    fn copy_ready(&self) {}
    fn restore_ready(&self) {}
    fn quit_answer(&self, _answer: crate::listener::QuitAnswer) {}
    fn hand_off_quit(&self) {}
}

/// Answer pending asks before scoped threads join, including after an assertion unwinds.
struct Cleanup<'a>(&'a QaBridge);

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        let ids: Vec<_> = lock(&self.0.answers).keys().copied().collect();
        for id in ids {
            self.0.answer(id, false, "test ended".to_owned());
        }
    }
}

/// Failure: the older ask's completion clears the newer ask's watchdog label while that request still waits for the host. Both success and host refusal must remove only their own ownership.
#[test]
fn an_older_qa_completion_preserves_the_newer_pending_verb() {
    #[cfg(target_os = "macos")]
    assert_eq!(std::mem::size_of::<QaBridge>(), 200, "fixed per-app state must not grow for per-request labels");
    let app = HeadlessApp::start();
    let inner = app.core();
    let (requests, heard) = mpsc::channel();
    let qa = QaBridge::new(Arc::new(Requests(requests)), inner.engine.clone(), Watchdog::new(), Arc::clone(&inner.core));
    for ok in [true, false] {
        std::thread::scope(|scope| {
            let _cleanup = Cleanup(&qa);
            let first = scope.spawn(|| qa.ask("state", ""));
            let (first_id, first_verb) = heard.recv_timeout(LIMIT).expect("first request admitted");
            assert_eq!(first_verb, "state");
            let second = scope.spawn(|| qa.ask("animations", ""));
            let (second_id, _) = heard.recv_timeout(LIMIT).expect("second request admitted");
            assert_eq!(qa.watchdog.current_verb().as_str(), "animations");
            qa.answer(first_id, ok, if ok { "null" } else { "fixture refusal" }.to_owned());
            assert_eq!(first.join().unwrap().is_ok(), ok);
            assert_eq!(
                qa.watchdog.current_verb().as_str(),
                "animations",
                "finishing an older request erased the pending newer verb"
            );
            qa.answer(second_id, true, "[]".to_owned());
            second.join().unwrap().unwrap();
            assert_eq!(qa.watchdog.current_verb().as_str(), "");
        });
    }
    drop(app.quit());
}

/// Failure: a timed-out newest request clears the older request's diagnostic label. The zero wait exercises the real ask timeout branch without waiting out the shipping timeout or asserting a machine's speed.
#[test]
fn a_qa_timeout_restores_the_other_pending_verb() {
    let app = HeadlessApp::start();
    let inner = app.core();
    let (requests, heard) = mpsc::channel();
    let qa = QaBridge::new(Arc::new(Requests(requests)), inner.engine.clone(), Watchdog::new(), Arc::clone(&inner.core));
    std::thread::scope(|scope| {
        let _cleanup = Cleanup(&qa);
        let first = scope.spawn(|| qa.ask("state", ""));
        let (first_id, _) = heard.recv_timeout(LIMIT).expect("first request admitted");
        let timeout = scope.spawn(|| qa.ask_with_wait("animations", "", Duration::ZERO));
        let (expired_id, _) = heard.recv_timeout(LIMIT).expect("timing-out request admitted");
        assert!(timeout.join().unwrap().is_err());
        assert_eq!(qa.watchdog.current_verb().as_str(), "state", "the other request is still pending after timeout");
        qa.answer(expired_id, true, "[\"late\"]".to_owned());
        assert_eq!(qa.watchdog.current_verb().as_str(), "state");
        qa.answer(first_id, true, "null".to_owned());
        first.join().unwrap().unwrap();
        assert!(lock(&qa.answers).is_empty(), "timeout and late answers leave no slots");
        assert_eq!(qa.watchdog.current_verb().as_str(), "");
    });
    drop(app.quit());
}

/// Failure: completing the newest ask clears the watchdog instead of restoring the older request that still waits for its reply.
#[test]
fn a_newer_qa_completion_restores_the_older_pending_verb() {
    let app = HeadlessApp::start();
    let inner = app.core();
    let (requests, heard) = mpsc::channel();
    let qa = QaBridge::new(Arc::new(Requests(requests)), inner.engine.clone(), Watchdog::new(), Arc::clone(&inner.core));
    std::thread::scope(|scope| {
        let _cleanup = Cleanup(&qa);
        let first = scope.spawn(|| qa.ask("state", ""));
        let (first_id, _) = heard.recv_timeout(LIMIT).expect("first request admitted");
        let second = scope.spawn(|| qa.ask("animations", ""));
        let (second_id, _) = heard.recv_timeout(LIMIT).expect("second request admitted");
        qa.answer(second_id, true, "[]".to_owned());
        second.join().unwrap().unwrap();
        assert_eq!(qa.watchdog.current_verb().as_str(), "state", "the older request is still pending");
        qa.answer(first_id, true, "null".to_owned());
        first.join().unwrap().unwrap();
        assert_eq!(qa.watchdog.current_verb().as_str(), "");
    });
    drop(app.quit());
}

/// Failure: consuming a reply drops its pending label before the ask finishes decoding. A later duplicate must also leave the consumed reply and its original outcome alone.
#[test]
fn a_qa_verb_stays_owned_through_decode_and_ignores_duplicate_answers() {
    use super::answer_lock_tests::{DECODING, DecodeHold};
    struct ReleaseDecode<'a>(&'a DecodeHold);
    impl Drop for ReleaseDecode<'_> {
        fn drop(&mut self) {
            self.0.release();
        }
    }

    let app = HeadlessApp::start();
    let inner = app.core();
    let (requests, heard) = mpsc::channel();
    let qa = QaBridge::new(Arc::new(Requests(requests)), inner.engine.clone(), Watchdog::new(), Arc::clone(&inner.core));
    let hold = Arc::new(DecodeHold::default());
    std::thread::scope(|scope| {
        let _cleanup = Cleanup(&qa);
        let release = ReleaseDecode(&hold);
        let older = scope.spawn(|| qa.ask("state", ""));
        let (older_id, _) = heard.recv_timeout(LIMIT).expect("older request admitted");
        let newer = scope.spawn(|| {
            DECODING.with(|slot| *slot.borrow_mut() = Some(Arc::clone(&hold)));
            qa.ask("animations", "")
        });
        let (newer_id, _) = heard.recv_timeout(LIMIT).expect("newer request admitted");
        qa.answer(newer_id, true, "[\"original\"]".to_owned());
        assert!(hold.entered(), "newer ask is decoding outside the map gate");
        qa.answer(newer_id, false, "late duplicate".to_owned());
        let duplicate = lock(&qa.answers).get_mut(&newer_id).expect("decoding retains its metadata slot").take();
        assert!(duplicate.is_none(), "a duplicate repopulated the consumed answer slot");
        qa.answer(older_id, true, "null".to_owned());
        older.join().unwrap().unwrap();
        assert_eq!(
            qa.watchdog.current_verb().as_str(),
            "animations",
            "the newer ask still owns its label while decoding"
        );
        drop(release);
        assert_eq!(newer.join().unwrap().unwrap(), json!(["original"]));
        assert_eq!(qa.watchdog.current_verb().as_str(), "");
        assert!(lock(&qa.answers).is_empty());
    });
    drop(app.quit());
}
