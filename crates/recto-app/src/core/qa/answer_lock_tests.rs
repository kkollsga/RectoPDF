//! A host answer must never wait for another QA job to decode JSON.

use super::*;
use crate::testkit::headless::HeadlessApp;
use std::cell::RefCell;
use std::sync::mpsc;

const LIMIT: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(super) struct DecodeHold {
    state: Mutex<(bool, bool)>, // entered, released
    changed: Condvar,
}

impl DecodeHold {
    fn wait(&self) {
        let mut state = lock(&self.state);
        state.0 = true;
        self.changed.notify_all();
        while !state.1 {
            state = self.changed.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
    }

    pub(super) fn entered(&self) -> bool {
        let (state, _) = self
            .changed
            .wait_timeout_while(lock(&self.state), LIMIT, |state| !state.0)
            .unwrap_or_else(PoisonError::into_inner);
        state.0
    }

    pub(super) fn release(&self) {
        lock(&self.state).1 = true;
        self.changed.notify_all();
    }
}

thread_local! {
    pub(super) static DECODING: RefCell<Option<Arc<DecodeHold>>> = const { RefCell::new(None) };
}

/// Only a test's first ask thread arms this; production has no seam or added state.
pub(super) fn before_decode() {
    DECODING.with(|held| {
        if let Some(hold) = held.borrow().as_ref() {
            hold.wait();
        }
    });
}

struct Requests(mpsc::Sender<u64>);

impl crate::Listener for Requests {
    fn model_changed(&self) {}
    fn ops_ready(&self) {}
    fn qa_request(&self, id: u64, _verb: String, _value: String) {
        let _ = self.0.send(id);
    }
    fn session_ended(&self) {}
    fn copy_ready(&self) {}
    fn restore_ready(&self) {}
    fn quit_answer(&self, _answer: crate::listener::QuitAnswer) {}
    fn hand_off_quit(&self) {}
}

/// Release before the scoped threads are joined, including when an assertion unwinds. A failed setup also answers any still-pending test slots.
struct Cleanup<'a> {
    hold: &'a DecodeHold,
    qa: &'a QaBridge,
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        self.hold.release();
        if std::thread::panicking() {
            let ids: Vec<_> = lock(&self.qa.answers).keys().copied().collect();
            for id in ids {
                self.qa.answer(id, false, "test ended".to_owned());
            }
        }
    }
}

/// Failure: decoding one host reply holds the answer map, so a second reply blocks the UI thread. Two actual asks have slots before the first decoder is held; the UI's second answer and its ask must finish while that hold remains closed.
#[test]
fn a_host_answer_does_not_wait_for_another_answers_json_decode() {
    let app = HeadlessApp::start();
    let inner = app.core();
    let (requests, heard) = mpsc::channel();
    let qa = QaBridge::new(Arc::new(Requests(requests)), inner.engine.clone(), Watchdog::new(), Arc::clone(&inner.core));
    let hold = Arc::new(DecodeHold::default());
    std::thread::scope(|scope| {
        let cleanup = Cleanup { hold: &hold, qa: &qa };
        let first = scope.spawn(|| {
            DECODING.with(|slot| *slot.borrow_mut() = Some(Arc::clone(&hold)));
            qa.ask("animations", "")
        });
        let first_id = heard.recv_timeout(LIMIT).expect("first ask reached the host");
        let (second_done, second_result) = mpsc::channel();
        let second_qa = &qa;
        let second = scope.spawn(move || {
            let result = second_qa.ask("animations", "");
            second_done.send(result).unwrap();
        });
        let second_id = heard.recv_timeout(LIMIT).expect("second ask has its answer slot");
        qa.answer(first_id, true, "{\"first\":1}".to_owned());
        assert!(hold.entered(), "first ask reached its held JSON decoder");
        let (returned, answer_returned) = mpsc::channel();
        let ui_qa = &qa;
        let ui = scope.spawn(move || {
            ui_qa.answer(second_id, true, "[\"second\"]".to_owned());
            returned.send(()).unwrap();
        });
        let delivered = answer_returned.recv_timeout(LIMIT);
        let second_while_held = delivered.is_ok().then(|| second_result.recv_timeout(LIMIT));
        // Capture what completed under the hold, then always release before joins or an assertion about those observations.
        drop(cleanup);
        let first = first.join().unwrap();
        ui.join().unwrap();
        second.join().unwrap();
        assert!(delivered.is_ok(), "the host answer waited for the held decoder");
        assert_eq!(second_while_held.unwrap().unwrap().unwrap(), json!(["second"]));
        assert_eq!(first.unwrap(), json!({ "first": 1 }));
    });

    // The same ask/answer route still preserves malformed success, explicit failure, and removal before any late duplicate answer.
    for (ok, text) in [(true, "not JSON"), (false, "fixture refusal")] {
        std::thread::scope(|scope| {
            let asked = scope.spawn(|| qa.ask("animations", ""));
            let id = heard.recv_timeout(LIMIT).expect("ask reached the host");
            qa.answer(id, ok, text.to_owned());
            let answer = asked.join().unwrap();
            if ok {
                assert_eq!(answer.unwrap(), Value::Null);
            } else {
                let refused = answer.unwrap_err();
                assert_eq!(refused.code, Code::QA_FAILED);
                assert_eq!(refused.message, text);
            }
            qa.answer(id, true, "[\"late\"]".to_owned());
            assert!(lock(&qa.answers).is_empty(), "a late answer cannot resurrect a removed slot");
        });
    }
    drop(app.quit());
}
