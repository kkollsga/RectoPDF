//! `recto qa budget`'s times: **the page drawn, and the window at rest, each on its own.**
//!
//! `qa wait-settled` waits for the whole app at rest, and that includes the window's animations (the page number's and zoom marks' fade, which since
//! 2026-11-08 runs about 190 ms after a change) and the looks that run behind a page (an edited file's signatures, seconds on a 2 GiB file). A budget
//! line that names the page sharp (`SPEC.md` section 2) has to stop at the page, so each line is read two ways:
//! - **drawn**: the first read of `qa state` that says the reader is settled with tiles attached (`reader.settled` and `reader.tiles.attached`: the page
//!   views' last jobs ended and a tile on show), read in a loop from the moment the action was sent. What it measures is the page, so an animation or a
//!   background look that is still going is not waited for. It is an upper bound to within one read, which the report says (`read_ms_p50`);
//! - **settled**: `qa wait-settled`'s answer, as before. The app at rest, which tests ask and a person doesn't see.
//!
//! Nothing here changes what settled means. The app stamps its own first frame (`record.first_shown.ms_from_start`), which is reported beside.

use recto_agent::{AgentError, Code, exit};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// How long a page may take to be drawn before the budget run says so, a minute as `qa wait-settled`'s own limit in the run.
pub const DRAWN_LIMIT: Duration = Duration::from_secs(60);

/// Whether a `qa state` reading has the page drawn: the reader settled and a tile on show. The same fields the review's probe polled
/// (`docs/reports/performance-2026-11-16-probe.py`), and nothing of the window's animations or of what runs behind the page.
pub fn page_drawn(state: &Value) -> bool {
    state["reader"]["settled"] == true && state["reader"]["tiles"]["attached"].as_u64().unwrap_or(0) > 0
}

/// When the page was drawn, and what the reads that found it cost.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drawn {
    /// Milliseconds from the start of the run's clock to the end of the first read that had the page drawn.
    pub at_ms: f64,
    /// Reads made, the last being the one that had it.
    pub reads: u32,
    /// The longest read, which is how late the time can be.
    pub longest_read_ms: f64,
}

/// Reads the state until the page is drawn. `clock` is milliseconds on the run's own clock, so a test can feed it. `read` is one `qa state`; the loop
/// pauses 1 ms between two, so it doesn't spin, and gives up after `limit_ms` of the clock.
pub fn until_drawn(
    limit_ms: f64,
    mut clock: impl FnMut() -> f64,
    mut read: impl FnMut() -> Result<Value, AgentError>,
    accept: fn(&Value) -> bool,
) -> Result<Drawn, AgentError> {
    let mut reads = 0u32;
    let mut longest: f64 = 0.0;
    loop {
        let before = clock();
        let state = read()?;
        let after = clock();
        reads += 1;
        longest = longest.max(after - before);
        if accept(&state) {
            return Ok(Drawn {
                at_ms: after,
                reads,
                longest_read_ms: longest,
            });
        }
        if after > limit_ms {
            return Err(AgentError::new(Code::TIMED_OUT, "The page wasn't drawn in time.", exit::TIMED_OUT)
                .then("`recto qa state` shows `reader.settled` and `reader.tiles`."));
        }
        std::thread::sleep(Duration::from_millis(1)); // ratchet: poll
    }
}

/// [`until_drawn`] on the real clock from `since`, reading with `read`.
pub fn drawn_since(since: Instant, read: impl FnMut() -> Result<Value, AgentError>, accept: fn(&Value) -> bool) -> Result<Drawn, AgentError> {
    let limit = since.elapsed().as_secs_f64() * 1000.0 + DRAWN_LIMIT.as_secs_f64() * 1000.0;
    until_drawn(limit, || since.elapsed().as_secs_f64() * 1000.0, read, accept)
}

/// The page drawn with the sidebar showing, for the line that times the sidebar coming back: the sidebar's rows are the host's, the page view beside it
/// is redrawn at its new width, and that's the drawing a sidebar opening causes.
pub fn sidebar_and_page_drawn(state: &Value) -> bool {
    state["app"]["sidebar_visible"] == true && page_drawn(state)
}

/// One measured moment: the page drawn, then the window at rest, both from the same start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Moment {
    pub drawn_ms: f64,
    pub settled_ms: f64,
    pub longest_read_ms: f64,
}

impl Moment {
    /// A moment from the two times. The window can't be at rest before the page is drawn, so a settled time under the drawn one is the drawn one
    /// (the page's read was the later look).
    pub fn new(drawn: Drawn, settled_ms: f64) -> Self {
        Self {
            drawn_ms: drawn.at_ms,
            settled_ms: settled_ms.max(drawn.at_ms),
            longest_read_ms: drawn.longest_read_ms,
        }
    }
}

/// The reported line for a set of moments: the page drawn (what the budget names) and the window settled, each as p50, p95 and the worst, and how
/// the two were taken. `extra` is what the line adds (its budget).
pub fn line(moments: &[Moment], extra: Value) -> Value {
    let sorted = |pick: fn(&Moment) -> f64| {
        let mut values: Vec<f64> = moments.iter().map(pick).collect();
        values.sort_by(f64::total_cmp);
        values
    };
    let (drawn, settled, reads) = (
        sorted(|moment| moment.drawn_ms),
        sorted(|moment| moment.settled_ms),
        sorted(|moment| moment.longest_read_ms),
    );
    let spread = |values: &[f64]| json!({ "p50": super::qa::round(super::qa::percentile(values, 0.5)), "p95": super::qa::round(super::qa::percentile(values, 0.95)), "max": super::qa::round(super::qa::percentile(values, 1.0)) });
    let mut line = json!({
        "runs": moments.len(),
        "page_drawn_ms": spread(&drawn),
        "window_settled_ms": spread(&settled),
        "read_ms_worst": super::qa::round(super::qa::percentile(&reads, 1.0)),
        "how": HOW,
    });
    if let (Some(line), Some(extra)) = (line.as_object_mut(), extra.as_object()) {
        line.extend(extra.clone());
    }
    line
}

/// What each time is, said in the output so neither is taken for the other.
pub const HOW: &str = "page_drawn_ms: from the action to the first `qa state` read with the reader settled and a tile on show, so the window's fades and the looks behind the page (an edited file's signatures) aren't in it; an upper bound to within one read (read_ms_worst). window_settled_ms: from the same start to `qa wait-settled` answering, which also waits for the window's animations and background looks: the app at rest, not the page.";

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn state(settled: bool, attached: u64) -> Value {
        json!({ "reader": { "settled": settled, "tiles": { "attached": attached } }, "app": { "animations": ["chrome_fading"] } })
    }

    /// Feeds `states` one per read, a clock that moves 5 ms a read, so the time of a read is known.
    fn feed(states: Vec<Value>) -> Result<Drawn, AgentError> {
        let reads = Cell::new(0usize);
        until_drawn(
            1000.0,
            || reads.get() as f64 * 2.5,
            || {
                let at = reads.get();
                reads.set(at + 1);
                Ok(states[at.min(states.len() - 1)].clone())
            },
            page_drawn,
        )
    }

    /// Failure: the page was timed to the window's rest. The record has the page drawn at the third read with the page number's fade still showing
    /// (`app.animations`) and then a background look (`reader` with a signatures look still under way, found late) going on; the time must be the third
    /// read's, not a later read where the fade has ended.
    #[test]
    fn the_page_is_timed_where_it_is_drawn_not_where_the_fade_or_a_background_look_ends() {
        let mut still = state(true, 3);
        still["reader"]["signatures_look"] = json!("under way");
        let drawn = feed(vec![
            state(false, 0),
            state(false, 0),
            state(true, 3),
            still.clone(),
            still.clone(),
            state(true, 3),
        ])
        .unwrap();
        assert_eq!(drawn.reads, 3, "the third read is the first with the page drawn: {drawn:?}");
        assert!(drawn.at_ms < 10.0, "it's the time of that read, not of the end of the fade: {drawn:?}");
    }

    /// Failure: a page with no tile on show was called drawn because the reader says settled (a document just opened has nothing to wait for).
    #[test]
    fn settled_with_no_tile_is_not_a_page() {
        assert!(!page_drawn(&state(true, 0)));
        assert!(!page_drawn(&state(false, 4)));
        assert!(page_drawn(&state(true, 1)));
        assert!(!page_drawn(&json!({})), "a reading with no reader says nothing is drawn");
    }

    /// Failure: a page never drawn made the run wait forever.
    #[test]
    fn a_page_that_never_draws_ends_in_a_timeout() {
        let at = Cell::new(0.0);
        let error = until_drawn(
            50.0,
            || {
                at.set(at.get() + 10.0);
                at.get()
            },
            || Ok(state(false, 0)),
            page_drawn,
        )
        .unwrap_err();
        assert_eq!(error.code, Code::TIMED_OUT);
    }

    /// Failure: the line gave one number, so a reader took the window's rest (445 ms on 2026-11-16) for the page (274). Both are in it, as their own spreads,
    /// the drawn never above the settled, and the output says how each was taken.
    #[test]
    fn the_line_has_both_times_apart_and_says_how() {
        let drawn = |at_ms| Drawn {
            at_ms,
            reads: 3,
            longest_read_ms: 4.0,
        };
        // A drawn time after settled's can't happen; the moment takes the drawn one.
        let moments = [
            Moment::new(drawn(270.0), 440.0),
            Moment::new(drawn(280.0), 450.0),
            Moment::new(drawn(300.0), 290.0),
        ];
        assert_eq!(moments[2].settled_ms, 300.0);
        let line = line(&moments, json!({ "budget_p50": 300 }));
        assert_eq!(line["page_drawn_ms"]["p50"], 280.0);
        assert_eq!(line["window_settled_ms"]["p50"], 440.0);
        assert_eq!(line["window_settled_ms"]["max"], 450.0);
        assert_eq!(line["budget_p50"], 300);
        assert!(line["how"].as_str().unwrap().contains("fades"));
    }
}
