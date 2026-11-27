//! **`qa wait-settled --timeout T`** keeps T as the bound on the app's *no progress*, not on the wall clock (2026-11-27: three of 30 lifecycle runs ended
//! "didn't settle in time" at 30 s on a machine at load 15 to 29, as `qa start` had at its 30 s). It asks the app to settle in slices; between them the
//! CLI reads the app's counters from its pid and applies `qa_launch`'s rule ([`progressed`]: CPU of a hundredth of T since the last mark, or any page-in).
//! It ends when the app made no progress for T, or a hard cap of 6 T (at least 3 minutes) has passed, whichever is first. The app never measures itself.

use crate::qa_launch::{Marks, Patience, Verdict, marks_of, progressed, ready_verdict};
use recto_agent::{AgentError, Code, exit};
use recto_control::client::Client;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// The most a wait lasts in all, as a multiple of the timeout asked and at least this.
const CAP_FACTOR: u32 = 6;
const CAP_LEAST: Duration = Duration::from_secs(180);

/// A wait's patience from the `--timeout` asked.
#[must_use]
pub fn patience_for(timeout: Duration) -> Patience {
    Patience {
        stall: timeout,
        cap: (timeout * CAP_FACTOR).max(CAP_LEAST),
    }
}

/// How long the app is asked to settle at a time: a quarter of the stall, so a stuck app is called within a quarter of it past.
fn slice_of(patience: Patience) -> Duration {
    (patience.stall / 4).clamp(Duration::from_millis(250), Duration::from_secs(5))
}

/// The refusal for a wait that ended, naming which bound it was. `last` is what the app last said it was waiting for.
#[must_use]
pub fn refusal(verdict: Verdict, elapsed: Duration, patience: Patience, last: Option<&str>) -> AgentError {
    let seconds = |span: Duration| span.as_secs_f64().round() as u64;
    let said = match verdict {
        Verdict::Capped => format!("The app didn't settle: still not settled after {} s.", seconds(elapsed)),
        _ => format!("The app didn't settle: it made no progress for {} s.", seconds(patience.stall)),
    };
    let said = last.map_or(said.clone(), |last| format!("{said} It last said: {last}"));
    AgentError::new(Code::TIMED_OUT, said, exit::TIMED_OUT).then("`recto qa state` shows what it's still waiting for.")
}

/// What the app said when it had not settled within the slice, as opposed to settled or another refusal.
fn not_settled_yet(answer: &Value) -> Option<String> {
    (answer["ok"] != true && answer["error"]["code"] == Code::TIMED_OUT.to_string().as_str())
        .then(|| answer["error"]["message"].as_str().unwrap_or("").to_string())
}

/// The wait. `ask(slice)` is the app's answer to "settle within `slice`"; `read` the app's counters now; `clock` the time since the wait began. A settled
/// app's answer, and any refusal that isn't a time-out, is returned as the app gave it.
pub fn wait(
    patience: Patience,
    mut ask: impl FnMut(Duration) -> Result<Value, AgentError>,
    mut read: impl FnMut() -> Option<Marks>,
    mut clock: impl FnMut() -> Duration,
) -> Result<Value, AgentError> {
    let mut mark = read();
    let mut last_progress = clock();
    loop {
        let slice = slice_of(patience).min(patience.cap.saturating_sub(clock())).max(Duration::from_millis(1));
        let answer = ask(slice)?;
        let Some(said) = not_settled_yet(&answer) else { return Ok(answer) };
        // Counters that can't be read say nothing about progress, so only the cap ends such a wait.
        match (mark.as_mut(), read()) {
            (Some(mark), Some(now)) => {
                if progressed(mark, now, patience) {
                    last_progress = clock();
                }
            }
            (_, now) => {
                mark = now;
                last_progress = clock();
            }
        }
        let elapsed = clock();
        match ready_verdict(elapsed, elapsed.saturating_sub(last_progress), patience) {
            Verdict::Wait => {}
            stop => return Err(refusal(stop, elapsed, patience, (!said.is_empty()).then_some(said.as_str()))),
        }
    }
}

/// `qa wait-settled --timeout` against a connected session: the app's pid from its status, then the wait.
pub fn through(client: &mut Client, timeout: Duration, client_wait: Duration) -> Result<Value, AgentError> {
    let pid = client
        .call(&json!({ "cmd": "status" }).to_string(), Duration::from_secs(5))
        .ok()
        .and_then(|status| status["data"]["pid"].as_u64())
        .and_then(|pid| u32::try_from(pid).ok());
    let began = Instant::now();
    wait(
        patience_for(timeout),
        |slice| {
            let line = json!({ "cmd": "qa_read", "verb": "settled", "timeout_ms": u64::try_from(slice.as_millis()).unwrap_or(u64::MAX) }).to_string();
            client.call(&line, client_wait + slice)
        },
        || pid.and_then(marks_of),
        || began.elapsed(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn timed_out() -> Value {
        json!({ "ok": false, "error": { "code": "timed_out", "message": "The app didn't settle in time: still under way, a look." } })
    }

    /// A fake app and clock: each ask takes its slice of time; counters are `marks_at(now)`; it settles at `settles_at`.
    fn run(timeout: Duration, settles_at: Duration, marks_at: impl Fn(Duration) -> Option<Marks>) -> Result<Value, AgentError> {
        let now = Cell::new(Duration::ZERO);
        wait(
            patience_for(timeout),
            |slice| {
                now.set(now.get() + slice);
                Ok(if now.get() >= settles_at {
                    json!({ "ok": true, "data": { "settled": true } })
                } else {
                    timed_out()
                })
            },
            || marks_at(now.get()),
            || now.get(),
        )
    }

    /// Failure: a settled app is refused. It answers at once, with no time-out passed.
    #[test]
    fn a_settled_app_answers_at_once() {
        let answered = run(Duration::from_secs(30), Duration::ZERO, |_| None).unwrap();
        assert_eq!(answered["data"]["settled"], true);
    }

    /// Failure: a wall clock. An app that keeps working (CPU every second) is waited for past `--timeout`, as a starved machine's is, and settles at 100 s with 30 s asked.
    #[test]
    fn an_app_that_makes_progress_is_waited_for_past_the_timeout() {
        let busy = |at: Duration| {
            Some(Marks {
                cpu_ns: at.as_nanos() as u64 / 2,
                page_ins: 0,
            })
        };
        let answered = run(Duration::from_secs(30), Duration::from_secs(100), busy).unwrap();
        assert_eq!(answered["data"]["settled"], true);
    }

    /// Failure: a hung app is waited for to the cap. One whose counters stand still ends at the timeout, and says it made no progress, not a missing flag.
    #[test]
    fn an_app_that_makes_no_progress_ends_at_the_timeout() {
        let still = |_| Some(Marks { cpu_ns: 5, page_ins: 0 });
        let error = run(Duration::from_secs(30), Duration::from_secs(10_000), still).unwrap_err();
        assert!(error.message.contains("made no progress for 30 s"), "{}", error.message);
        assert!(!error.message.contains("--"), "{}", error.message);
    }

    /// Failure: an app busy for ever, as a loop is, is waited for for ever. The cap (6 times the timeout, at least 3 minutes) ends it and says so.
    #[test]
    fn a_busy_app_is_ended_at_the_cap() {
        let busy = |at: Duration| {
            Some(Marks {
                cpu_ns: at.as_nanos() as u64,
                page_ins: 0,
            })
        };
        let error = run(Duration::from_secs(60), Duration::from_secs(100_000), busy).unwrap_err();
        assert!(error.message.contains("still not settled after 360 s"), "{}", error.message);
        let error = run(Duration::from_secs(5), Duration::from_secs(100_000), busy).unwrap_err();
        assert!(error.message.contains("still not settled after 180 s"), "{}", error.message);
    }

    /// Failure: counters that can't be read end the wait at the timeout, as a hung app. Only the cap does.
    #[test]
    fn with_no_counters_only_the_cap_ends_it() {
        let error = run(Duration::from_secs(30), Duration::from_secs(100_000), |_| None).unwrap_err();
        assert!(error.message.contains("still not settled after 180 s"), "{}", error.message);
    }

    /// Failure: another refusal (a level's, a bad verb) is taken for "not settled yet" and waited out. It is returned as it came.
    #[test]
    fn another_refusal_is_returned_at_once() {
        let answered = wait(
            patience_for(Duration::from_secs(30)),
            |_| Ok(json!({ "ok": false, "error": { "code": "permission_denied", "message": "no" } })),
            || None,
            || Duration::ZERO,
        )
        .unwrap();
        assert_eq!(answered["error"]["code"], "permission_denied");
    }
}
