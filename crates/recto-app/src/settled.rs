//! **The app at rest: the one answer to "has it settled?"**, which the headless kit's `settle`, `recto qa wait` and a shot's wait all ask here
//! (rule 4: four faults of this class in two days, 2026-11-13 and 14, each a "settled" answered by one of three ways of asking before what it waited for had
//! happened). For tests and QA only: nothing on a person's path asks it, and it never runs on the UI thread.
//!
//! **What's at rest.** Everything at once, seen twice in a row with nothing taken between the two looks:
//! - **the session thread**, by its own account ([`crate::queue::CoreQueue::at_rest`]): nothing queued for it, and parked having found none of its deadlines
//!   due now (the record's write, a tab's dormancy, a try at the record's lock, a restore's or the launch's patience), not nudged since and not past the time it
//!   parked till. A deadline due later (a cool-down, a retry in seconds) is not under way: on the kit's clock it's exact, as only the test moves that clock;
//! - **what runs off it and comes back to it**, each counted until after it has queued what it found: files being read, a password being tried, the person's
//!   commands on threads of their own and agents' commands in flight, a try at the record's lock out or a late keep's read that the person's commands are
//!   held for ([`crate::core::Lifecycle::keeping_thread_out`]), the window's saves, a snapshot on its way to the clipboard, a print's pages being
//!   prepared, the looks at other tabs' and woken tabs' files, at a file's signatures and at generated contents, Find's thread, and the record's writer
//!   running or a write due now;
//! - **the page views**, by whether a host draws them ([`PageViews`]): each one's last job's end told ([`recto_reader::Engine::wait_settled`]), and either drawn
//!   as far as it shows (a window) or its thread resting with everything asked of it seen and nothing left for the host to take (the kit, which has no
//!   view to draw).
//!
//! **What's left out, and why.** The sleep files' writer ([`crate::sleep_queue::Leaving`]): it queues nothing for the session, so it can't leave the session
//! unsettled, and a test that reads a sleep file or its counts waits for that write by name (some settle on purpose with it held mid-write). A late keep's read
//! once its restore's patience is over (`LettingGo`): nothing waits on it then, and the lock it keeps till it's out is waited for by name. A test that holds
//! a try or a late keep's read at its seam waits for the rest with `settled_but_the_held_thread` (`late_lock`'s). A test that holds a page view's thread (`Seam::Worker`) waits for the session's part with the host's turns (`host_turns`), as the kit's settle
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
//! **Which of a page view's jobs.** All of them, drawing included: in the kit, which has no viewport, they're a page's size (which says the file's length),
//! its links and text, and the pictures of a tab leaving the screen; under a window, what's in view. Measured on 2026-11-14 in the slowest test of the
//! package (`menu_twins`, 3,268 settles), the page views' share of its waits was nothing; Find's thread and the signature looks were most of it.
//!
//! **How the wait ends.** Each round: the session thread asked to get through what's queued (a flush, which also waits for files being read; skipped when the
//! last look found it at rest), then waited for as it parks, the host's turn (the kit takes the page views' layer changes as the window would), then the two
//! looks. Between rounds it waits on what it found under way, woken as that comes to rest: the session thread as it parks, a page view's thread as it rests,
//! and work off the session thread as the session thread takes its word and parks again; each wait at most a short timer (2 ms, growing to 16 ms for `qa
//! wait`), so a word that falls between a look and a wait costs no more. Measured on 2026-11-14, the package's slowest tests under `--like-ci` went from 162 to
//! 135 test-seconds with these waits, where the same rounds on timers alone took 490 against main's 424. It answers only on two clean looks, so never early;
//! and every wait in it is bounded by the caller's limit, past which it says what was still under way, so it can't hang.

use crate::core::Lifecycle;
use crate::core::saver::Saver;
use crate::core::sleeper::Sleeper;
use crate::core::{AppHost, Tools};
use crate::panes::Panes;
use crate::queue::CoreQueue;
use recto_reader::Engine;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// How often a wait looks again while something's under way, at first.
const LOOK_EVERY: Duration = Duration::from_millis(2);
/// How far `qa wait`'s pause between looks grows while something stays under way (a long Find, a look at a file on a disk that's stopped answering): the
/// app it waits on isn't looked at every 2 ms for its whole limit. The kit's stays at [`LOOK_EVERY`], as its tests wait for what's under way to end at once.
const LOOK_AT_MOST_EVERY: Duration = Duration::from_millis(16);

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
            sleeper: $inner.on_disk.sleeper(),
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
    /// The session thread was at rest at this look: the next round needn't ask it to get through its queue.
    session_at_rest: bool,
    /// Every page view was at rest at this look.
    views_at_rest: bool,
}

/// **Waits for the app to be at rest** (the module's notes), at most `limit`, running `host` each round as the host's own turn. Says what was still under way
/// if it wasn't by then.
///
/// # Errors
///
/// Past `limit`, with what was still under way at the last look.
pub fn wait(inner: &Watched<'_>, views: PageViews, limit: Duration, mut host: impl FnMut()) -> Result<(), Vec<String>> {
    let began = Instant::now();
    let (mut pause, mut session_at_rest) = (LOOK_EVERY, false);
    loop {
        let left = limit.saturating_sub(began.elapsed());
        // Asked to get through its queue only when the last look found it at work: a flush is a call on the app's own session thread.
        if !session_at_rest && !inner.core.flush(left) {
            let mut busy = vec!["the session thread didn't get through what was queued, with the files being read".to_string()];
            busy.extend(look(inner, views).busy);
            return Err(busy);
        }
        // The session thread through the flush's own call, and parked: woken as it parks, not looked at on a timer.
        inner.core.wait_parked(limit.saturating_sub(began.elapsed()));
        if views == PageViews::Drawn {
            // Waited for where it's cheap to: each view's own condition, woken as its jobs end.
            for engine in inner.panes.engines(inner.engine) {
                engine.wait_settled(limit.saturating_sub(began.elapsed()));
            }
        }
        host();
        let parks = inner.core.parks();
        let first = look(inner, views);
        let views_at_rest = first.views_at_rest;
        session_at_rest = first.session_at_rest;
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
        // Waited for by what it waits on, woken as that comes to rest, where it can be: the session thread as it parks, a page view's thread as it rests;
        // what's left (a thread off the session thread, a page view's drawing for a window) on a short timer.
        let wait = pause.min(limit.saturating_sub(began.elapsed()));
        if !session_at_rest {
            inner.core.wait_parked(wait);
        } else if !views_at_rest {
            wait_for_the_views(inner, views, wait);
        } else {
            // What runs off the session thread mostly ends by queuing its word, which the session thread takes and then parks: woken by that.
            inner.core.wait_next_park(parks, wait);
        }
        if views == PageViews::Drawn {
            pause = (pause * 2).min(LOOK_AT_MOST_EVERY);
        }
    }
}

/// The page views, each woken as it comes to rest, for at most `wait`: in the kit its thread resting, under a window drawn as far as it shows.
fn wait_for_the_views(inner: &Watched<'_>, views: PageViews, wait: Duration) {
    for engine in inner.panes.engines(inner.engine) {
        match views {
            PageViews::Drawn => {
                engine.wait_settled(wait);
            }
            #[cfg(any(test, feature = "testkit"))]
            PageViews::AtRest => {
                engine.qa().wait_resting(wait);
            }
        }
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
    let session_at_rest = match inner.core.at_rest() {
        Ok(turns) => {
            stamp.push(turns);
            true
        }
        Err(what) => {
            busy.push(what.to_string());
            false
        }
    };
    off_the_session_thread(inner, &mut busy);
    let before_views = busy.len();
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
    let views_at_rest = busy.len() == before_views;
    Look {
        busy,
        stamp,
        session_at_rest,
        views_at_rest,
    }
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
    // A look at where a tab's generated contents stand, or at a shown tab's file, holding its way back to the session thread till it's queued what it found.
    let ways_back = inner.core.ways_back_held();
    if ways_back > 0 {
        busy.push(format!("{ways_back} look(s) holding a way back to the session thread"));
    }
    // An agent's command in flight on another connection (`qa` commands only ask, so `qa wait` never counts itself). A command that waits (`recto wait`)
    // counts for as long as it waits: `qa wait` beside one waits for it too.
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
    // A try at the record's lock out, or a late keep's read that what the person did is held for: each tells the session thread what it found, then the
    // life hears it.
    if inner.life.keeping_thread_out() {
        busy.push("a try at the record's lock, or a late keep's read, out on its thread".to_string());
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

    /// Failure (review of `build/settled-gaps`, 2026-11-14): a late keep's saved session, read on the try's thread, wasn't counted, so settle could answer
    /// with the person's commands held for it (the session thread keeps them till the restore's in). Held in that read: it's under way.
    #[test]
    fn a_late_keeps_read_out_isnt_settled() {
        let disk = Disk::new();
        let before = recto_control::record_store::RecordStore::open(disk.paths(), recto_control::record_store::Flavor::App);
        assert!(before.holds_the_record(), "the copy before keeps the record");
        let app = disk.builder().another_keeps().start();
        gate::hold(&[Seam::State]);
        drop(before);
        app.advance(crate::lifecycle::keeping::TRY_EVERY_MS);
        app.wait_until("the late keep's read held", || app.waiting(Seam::State) > 0);
        let busy = under_way(&super::watched!(app.core()), PageViews::AtRest);
        gate::open(Seam::State);
        app.wait_until("the read's thread back", || !app.core().life.keeping_thread_out());
        app.settle();
        assert!(
            busy.iter().any(|what| what.contains("record's lock")),
            "settled with the late keep's read out: {busy:?}"
        );
    }

    /// Failure (the settled audit, 2026-11-14, gap 2): `qa wait` flushed the session thread's queue and then waited for the page views, so a job that ended
    /// during that wait told the session its word (a page's note, a look) after the flush, and `qa wait` answered settled with it queued and not taken.
    /// Forced, as `qa wait` asks it ([`super::wait`] with a host that draws), from a thread of its own: its flush taken, then a page view's job ends held
    /// before its word ([`recto_reader::engine::Qa::hold_tells`]), the session thread held, and the word let go into its queue while the test plays the
    /// host. Settled must not be said with the session thread not through that word: here it's never through it inside the wait's limit, so the wait says
    /// what was still under way.
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
        // The scroll's own look at the session (a page moved) taken first; one that comes after this only sends the test down the not-settled path.
        app.wait_until("the session thread at rest", || app.core().core.at_rest().is_ok());
        let turns = app.core().core.at_rest().unwrap_or(0);
        let core = std::sync::Arc::clone(app.core());
        let (said, heard) = std::sync::mpsc::channel();
        let waiting = std::thread::Builder::new()
            .name("test-qa-wait".into())
            .spawn(move || {
                let answer = super::wait(&super::watched!(core), PageViews::Drawn, Duration::from_millis(500), || {});
                let _ = said.send((answer, core.core.at_rest().is_ok()));
            })
            .expect("a thread");
        app.wait_until("the wait's flush taken", || app.core().core.at_rest().is_ok_and(|now| now > turns));
        let held = app.hold_session_thread();
        engine.qa().hold_tells(false);
        // The host's turns, as the window takes the page view's layers, until the wait answers.
        let (answer, session_at_rest) = loop {
            applied_as_the_host(&engine.drain());
            engine.applied();
            if let Ok(said) = heard.recv_timeout(Duration::from_millis(5)) {
                break said;
            }
        };
        drop(held);
        waiting.join().expect("the wait's thread");
        host_turns(&app, &mut view, true);
        app.settle();
        // Any answer that isn't settled is right here: on a starved machine the wait may run out its limit on something else first.
        if answer.is_ok() {
            assert!(session_at_rest, "settled with the page view's word not taken by the session thread");
        }
    }
}
