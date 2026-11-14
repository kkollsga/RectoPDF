//! **The app at rest: the one answer to "has it settled?"**, which the headless kit's `settle`, `recto qa wait` and a shot's wait all ask here
//! (rule 4: four faults of this class in two days, 2026-11-13 and 14, each a "settled" answered by one of three ways of asking before what it waited for had
//! happened). For tests and QA only: nothing on a person's path asks it, and it never runs on the UI thread.
//!
//! **What's at rest.** Everything at once, seen twice in a row with nothing taken between the two looks:
//! - **the session thread**, by its own account ([`crate::queue::CoreQueue::at_rest`]): nothing queued for it, and parked having found none of its deadlines
//!   due now (the record's write, a tab's dormancy, a try at the record's lock, a restore's or the launch's patience), not nudged since and not past the time it
//!   parked till. A deadline due later (a cool-down, a retry in seconds) is not under way: on the kit's clock it's exact, as only the test moves that clock;
//! - **what runs off it and comes back to it**, each counted until after it has queued what it found: files being read, a password being tried, the person's
//!   commands on threads of their own and agents' commands in flight, the window's saves, a snapshot on its way to the clipboard, a print's pages being
//!   prepared, the looks at other tabs' and woken tabs' files, at a file's signatures and at generated contents, Find's thread, and the record's writer
//!   running or a write due now;
//! - **the page views**, by whether a host draws them ([`PageViews`]): each one's last job's end told ([`recto_reader::Engine::wait_settled`]), and either drawn
//!   as far as it shows (a window) or its thread resting with everything asked of it seen (the kit, which has no view to draw).
//!
//! **What's left out, and why.** The sleep files' writer ([`crate::sleep_queue::Leaving`]): it queues nothing for the session, so it can't leave the session
//! unsettled, and a test that reads a sleep file or its counts waits for that write by name (some settle on purpose with it held mid-write). A late try at the
//! record's lock and a late keep's read, each on a thread of its own: the deadline that starts a try is counted (the session thread acts on it before it's at
//! rest), the thread it starts isn't, as tests hold it on purpose and wait for what it decides by name, and the life's keeping isn't the session's to read
//! from here. A test that holds a page view's thread (`Seam::Worker`) waits for the session's part with the host's turns (`host_turns`), as the kit's settle
//! waits for the page view at rest. The updater's check and install, each on a thread of its own: in the real app a check is a network fetch, and
//! installing or waiting lasts until a hand-off, so counting them would have `qa wait` wait those out; the kit's update tests wait for the updater by its
//! status, by name (`wait_for_status`), and its end says only that the window should look again. The search index, which the kit has off and which says
//! when it's done in its own words. In the window, its animations and a print's file being handed to the system, which `qa wait` adds itself, as only the
//! host knows them.
//!
//! **Why two looks.** The parts hand work to each other: a page view's job ends and queues the session a look; the session's call asks a page view for a job;
//! a read's thread ends and queues what it read. Each part is looked at in turn, so one look can see each at rest at a different moment with work passed
//! between them unseen. Every hand-over moves a count the look takes (the session thread's turns, a page view's asks and turns) or is a part not yet at rest
//! (a thread counted as running pushes what it found before it stops counting itself): so two looks at rest with the same counts saw a moment when all of it
//! was at rest at once.
//!
//! **How the wait ends.** Each round: the session thread asked to get through what's queued (a flush, which also waits for files being read), the host's turn
//! (the kit takes the page views' layer changes as the window would), then the two looks; otherwise a short pause and again. It answers only on two clean
//! looks, so never early; and every wait in it is bounded by the caller's limit, past which it says what was still under way, so it can't hang.

use crate::core::Lifecycle;
use crate::core::saver::Saver;
use crate::core::sleeper::Sleeper;
use crate::core::{AppHost, Tools};
use crate::panes::Panes;
use crate::queue::CoreQueue;
use recto_reader::Engine;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// How often a wait looks again while something's under way.
const LOOK_EVERY: Duration = Duration::from_millis(2);

/// **What the page views are at rest by,** which depends on whether there's a host drawing them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageViews {
    /// A host draws them (the window, and `qa wait`): each one drawn as far as it shows, its layers applied by the host, and its last job's end told. Drawing
    /// for a tab off screen's place isn't waited for, as it shows nothing (see [`recto_reader::Engine::wait_settled`]).
    Drawn,
    /// No host draws them (the headless kit, which has no viewport): each one's thread resting, with no job running, everything asked of it seen and its last
    /// job's end told ([`recto_reader::engine::Qa::at_rest`]).
    #[cfg(any(test, feature = "testkit"))]
    AtRest,
}

/// **The parts of the app whose rest this looks at,** and nothing else of it: borrowed for one wait by whoever asks (the kit's `settle`, `qa wait`), each
/// field the app's own part of the same name.
pub struct Watched<'a> {
    pub core: &'a CoreQueue,
    pub life: &'a Lifecycle,
    pub tools: &'a Tools,
    pub sleeper: &'a Sleeper,
    pub saver: &'a Saver,
    pub host: &'a AppHost,
    pub panes: &'a Panes,
    pub engine: &'a Engine,
}

/// The parts of `$inner` (the app's core, or what derefs to it) that [`Watched`] borrows: written once, for each caller.
macro_rules! watched {
    ($inner:expr) => {
        $crate::settled::Watched {
            core: &$inner.core,
            life: &$inner.life,
            tools: &$inner.tools,
            sleeper: &$inner.sleeper,
            saver: &$inner.saver,
            host: &$inner.host,
            panes: &$inner.panes,
            engine: &$inner.engine,
        }
    };
}
pub(crate) use watched;

/// One look: what's under way, in words, or nothing; and the counts that move whenever work is handed on, for the next look to compare.
struct Look {
    busy: Vec<String>,
    stamp: Vec<u64>,
}

/// **Waits for the app to be at rest** (the module's notes), at most `limit`, running `host` each round as the host's own turn. Says what was still under way
/// if it wasn't by then.
///
/// # Errors
///
/// Past `limit`, with what was still under way at the last look.
pub fn wait(inner: &Watched<'_>, views: PageViews, limit: Duration, mut host: impl FnMut()) -> Result<(), Vec<String>> {
    let began = Instant::now();
    loop {
        let left = limit.saturating_sub(began.elapsed());
        if !inner.core.flush(left) {
            let mut busy = vec!["the session thread didn't get through what was queued, with the files being read".to_string()];
            busy.extend(look(inner, views).busy);
            return Err(busy);
        }
        if views == PageViews::Drawn {
            // Waited for where it's cheap to: each view's own condition, woken as its jobs end.
            for engine in inner.panes.engines(inner.engine) {
                engine.wait_settled(limit.saturating_sub(began.elapsed()));
            }
        }
        host();
        let first = look(inner, views);
        let last = if first.busy.is_empty() {
            let second = look(inner, views);
            if second.busy.is_empty() && second.stamp == first.stamp {
                return Ok(());
            }
            second
        } else {
            first
        };
        if began.elapsed() >= limit {
            return Err(if last.busy.is_empty() {
                vec!["work handed on between every two looks".to_string()]
            } else {
                last.busy
            });
        }
        std::thread::sleep(LOOK_EVERY.min(limit.saturating_sub(began.elapsed()))); // ratchet: poll
    }
}

/// **What's under way now,** in words, from one look: nothing when everything is at rest at this look (one look is not yet settled: see [`wait`]).
#[must_use]
pub fn under_way(inner: &Watched<'_>, views: PageViews) -> Vec<String> {
    look(inner, views).busy
}

fn look(inner: &Watched<'_>, views: PageViews) -> Look {
    let mut busy = Vec::new();
    let mut stamp = Vec::new();
    match inner.core.at_rest() {
        Ok(turns) => stamp.push(turns),
        Err(what) => busy.push(what.to_string()),
    }
    off_the_session_thread(inner, &mut busy);
    for (at, engine) in inner.panes.engines(inner.engine).iter().enumerate() {
        match views {
            PageViews::Drawn => {
                if !engine.wait_settled(Duration::ZERO) {
                    busy.push(format!("page view {at} not yet drawn as far as it shows, or its last job's end not yet told"));
                }
            }
            #[cfg(any(test, feature = "testkit"))]
            PageViews::AtRest => match engine.qa().at_rest() {
                Some((turns, asks)) => stamp.extend([turns, asks]),
                None => busy.push(format!(
                    "page view {at}'s thread with a job running, something asked of it not yet seen, or its last job's end not yet told"
                )),
            },
        }
    }
    Look { busy, stamp }
}

/// What runs off the session thread and comes back to it, still going: each counts itself running until after it has queued what it found.
fn off_the_session_thread(inner: &Watched<'_>, busy: &mut Vec<String>) {
    if crate::lock(&inner.tools.prompts().passwords).trying() {
        busy.push("a password being tried".to_string());
    }
    let reads = inner.life.reads_running();
    if reads > 0 {
        busy.push(format!("{reads} file(s) being read"));
    }
    let persons = inner.life.persons_running();
    if persons > 0 {
        busy.push(format!("{persons} of the person's command(s) on a thread of their own"));
    }
    // The window's Save and Save As run on a thread of their own, and what they did is announced once they're through.
    let saves = inner.tools.save_ui().running.load(Ordering::SeqCst);
    if saves > 0 {
        busy.push(format!("{saves} of the window's save(s)"));
    }
    // A snapshot on its way to the clipboard, and a print's pages being prepared: each says what came of it, then stops counting.
    if inner.tools.tool().busy() {
        busy.push("a snapshot on its way to the clipboard".to_string());
    }
    if inner.tools.printing().preparing() {
        busy.push("a print's pages being prepared".to_string());
    }
    // A look at where a tab's generated contents stand, holding its way back to the session thread till it's queued what it found.
    if inner.tools.contents_looks().load(Ordering::SeqCst) > 0 {
        busy.push("a look at generated contents".to_string());
    }
    // An agent's command in flight on another connection (`qa` commands only ask, so `qa wait` never counts itself).
    let commands = inner.host.running.load(Ordering::SeqCst);
    if commands > 0 {
        busy.push(format!("{commands} command(s) in flight"));
    }
    // A first edit's look at the file's signatures reads the whole file, then has the session look again for Save's state.
    let signatures = inner.tools.save_ui().signature_looks();
    if signatures > 0 {
        busy.push(format!("{signatures} look(s) at a file's signatures"));
    }
    if inner.life.others_looking() {
        busy.push("the other tabs' files being looked at".to_string());
    }
    let looks = inner.sleeper.looks_running();
    if looks > 0 {
        busy.push(format!("{looks} look(s) at a woken tab's file"));
    }
    // Running, or due now and not yet started (the session thread starts it when it next looks at its deadlines).
    if inner.saver.is_writing() {
        busy.push("the record being written".to_string());
    }
    // Its thread goes once it's said how its last query ended, which the session thread takes as Find's news.
    if inner.host.session.finder().has_thread() {
        busy.push("Find looking".to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::{PageViews, under_way};
    use crate::testkit::headless::{Disk, HeadlessApp, WAIT_LIMIT, applied_as_the_host, host_turns};
    use recto_platform::slow::{Seam, gate};
    use std::time::Duration;

    /// Failure (the settled audit, 2026-11-14, gap 3): the kit's settle counted a write that was due and not started, and no other deadline of the session
    /// thread's: with a try at the record's lock, a tab's dormancy or a restore's patience due now, its flush was taken first (the session thread takes a
    /// queued call before it looks at its deadlines), nothing else was under way, and a test doing `advance(); settle(); assert` read before the session
    /// thread had acted on it. Forced: the session thread woken for a due try and held there, before it acts ([`Seam::Due`]); a look then must say so.
    #[test]
    fn a_deadline_due_and_not_yet_acted_on_isnt_settled() {
        let disk = Disk::new();
        let before = recto_control::record_store::RecordStore::open(disk.paths(), recto_control::record_store::Flavor::App);
        assert!(before.holds_the_record(), "the copy before keeps the record");
        let app = disk.builder().another_keeps().start();
        gate::hold(&[Seam::Due]);
        app.advance(crate::lifecycle::keeping::TRY_EVERY_MS);
        app.wait_until("the session thread woken for its due try", || app.waiting(Seam::Due) > 0);
        let busy = under_way(&crate::settled::watched!(app.core()), PageViews::AtRest);
        gate::open(Seam::Due);
        app.settle();
        drop(before);
        assert!(
            busy.iter().any(|what| what.contains("deadline")),
            "settled with a try at the record's lock due and not made: {busy:?}"
        );
    }

    /// Failure (the settled audit, 2026-11-14, gap 2): `qa wait` flushed the session thread's queue and then waited for the page views, so a job that ended
    /// during that wait told the session its word (a page's note, a look) after the flush, and `qa wait` answered settled with it queued and not taken. Forced:
    /// a page view's job ends held before its word ([`recto_reader::engine::Qa::hold_tells`]), the session thread held, and the word let go into its queue;
    /// the page view now says settled, as `qa wait` last asked it, and the one definition must not.
    #[test]
    fn a_page_views_word_the_session_hasnt_taken_isnt_settled() {
        let app = HeadlessApp::start();
        let doc = app.disk().pdf("a.pdf", 3);
        app.open(&doc);
        let engine = app.core().engine.clone();
        let mut view = recto_reader::Viewport {
            x: 0.0,
            y: 0.0,
            width: 292.0,
            height: 225.0,
            frame_width: 292.0,
            magnification: 1.0,
            backing: 1.0,
            live: false,
            scrolled_to: u64::MAX,
        };
        applied_as_the_host(&engine.set_viewport(view));
        host_turns(&app, &mut view, true);
        engine.qa().hold_tells(true);
        view.y += 300.0;
        applied_as_the_host(&engine.set_viewport(view));
        assert!(engine.qa().wait_for_held_tell(WAIT_LIMIT), "a job's end held before its word");
        let held = app.hold_session_thread();
        engine.qa().hold_tells(false);
        let mut drawn = false;
        let began = std::time::Instant::now();
        while !drawn {
            applied_as_the_host(&engine.drain());
            engine.applied();
            drawn = engine.wait_settled(Duration::from_millis(5));
            assert!(began.elapsed() < WAIT_LIMIT, "the page view never came to rest: {}", engine.report().0);
        }
        let busy = under_way(&crate::settled::watched!(app.core()), PageViews::Drawn);
        drop(held);
        host_turns(&app, &mut view, true);
        app.settle();
        assert!(
            busy.iter().any(|what| what.contains("session thread")),
            "settled with the page view's word not taken by the session: {busy:?}"
        );
    }
}
