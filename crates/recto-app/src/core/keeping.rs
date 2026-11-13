//! **The record's lock come late, carried out:** what [`crate::lifecycle::keeping`] decides (a launch that didn't get the lock tries again quietly for ten
//! seconds), done. Every try is the store's one `try_lock` ([`recto_control::record_store::RecordStore::try_to_keep`]) on the session thread's own deadline,
//! with nothing held across it; the UI thread never waits for any of it.
//!
//! **The order, once a try gets the lock** (the review of 2026-11-13's two blockers):
//! 1. The try is made holding the saver's gate and the life's lock, so no writer of the record is between gathering what it writes and writing it, and no quit
//!    or hand-off begins between the look at whether a try is due and the try; and with no agent's change under way. A writer holding the gate, or an agent's
//!    change under way, makes it a try that didn't get the lock: the session thread never waits for either. The machine is then restoring, and from here until the saved session is in, every writer writes nothing (`saver`) and what the person does waits,
//!    as at a launch (the session thread holds it back), so nothing they do is replaced by the restore.
//! 2. Off the session thread, as at a launch ([`super::launching::read_state`]): the updater is told of the launch, held back until now, as a copy that keeps the
//!    record ([`updates::tell_launch`]), before anything is read, so a version that keeps failing goes back here having written nothing over the record; then
//!    the saved session is read, fallen back from if it can't be read after an update that kept ending, and its tabs counted.
//! 3. On the session thread, as at a launch: it's taken in ([`super::session::apply_restored`]), the machine keeps the record, what the person did meanwhile
//!    runs on it, a first frame shown meanwhile is told to the updater with the tabs it came up with, and the host is told to make the window again from it,
//!    once, if the person hasn't touched it.
//!
//! The read itself is about 2.3 ms for a session of 100 tabs each with a journal (median of 20, release build, this Mac under a load of about 4, 2026-11-13), but
//! telling the updater of a launch can tidy up what an install left beside the app, so neither is done on the session thread.

use super::*;
use crate::lifecycle::Event as LifeEvent;
use crate::lifecycle::keeping::{Event as KeepingEvent, Out as KeepingOut};
use std::hash::{Hash as _, Hasher as _};

/// **What the window has now, as one number:** the session as the record would have it, the appearance the person chose, and any file being opened. What
/// counts and what doesn't is said in [`crate::lifecycle::keeping`]: the sidebar shown or hidden is the window's arrangement, so it's left out here, with the
/// window's frame, focus, the sidebar's width, the panel and Settings, which aren't in the session's record at all.
fn as_done(inner: &Inner, reads: &FileReads) -> u64 {
    let mut record = inner.host.session.with_session(|session| session.to_record());
    record.view.sidebar_hidden = false;
    let appearance = lock(&inner.window.app).appearance;
    let mut opening: Vec<&std::path::Path> = reads
        .running
        .iter()
        .filter_map(|reading| match reading {
            Reading::File(path) => Some(path.as_path()),
            _ => None,
        })
        .collect();
    opening.sort_unstable();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_vec(&(record, appearance)).unwrap_or_default().hash(&mut hasher);
    opening.hash(&mut hasher);
    hasher.finish()
}

/// **The state's been taken in** (`session::apply_restored`), with `other_instance` when it was refused for the lock. The first time, whether this copy keeps
/// the record starts here; taken in with the lock while it's trying or restoring (a late try's restore, or any other read), it keeps it from here. Says whether
/// the notice that came with what was read is said now (not while it's trying), and what the host is told: the window made again from a late keep's restore.
pub(super) fn taken_in(inner: &Arc<Inner>, other_instance: bool, first_time: bool, reads: &FileReads) -> (bool, Option<crate::lifecycle::Out>) {
    let keeps = inner.saver.store.holds_the_record();
    let mut host = None;
    if first_time || keeps {
        let as_read = if keeps { 0 } else { as_done(inner, reads) };
        host = step(inner, KeepingEvent::Read { keeps, as_read });
    }
    (!(other_instance && inner.life.keeping().trying()), host)
}

/// After every call on the session thread, while it's trying: the person having opened or changed something stops it, and that's said now.
pub(super) fn look_after_call(inner: &Arc<Inner>, reads: &FileReads) {
    if !inner.life.keeping().trying() {
        return;
    }
    let now_as = as_done(inner, reads);
    if step(inner, KeepingEvent::Looked { now_as }).is_some() {
        inner.core.push(CoreCall::Reconcile);
    }
}

/// **The session thread's deadline while it's trying:** a look at the window, and if it's as it was read, one try at the lock sent out on a thread of its own
/// (see the module's order). With no thread to be had, it's a try that didn't get the lock.
pub(super) fn when_due(inner: &Arc<Inner>, reads: &FileReads) {
    if !inner.life.keeping_due().is_some_and(|wait| wait.is_zero()) {
        return;
    }
    let now_as = as_done(inner, reads);
    if step(inner, KeepingEvent::Looked { now_as }).is_some() || !inner.life.keeping().trying() {
        inner.core.push(CoreCall::Reconcile);
        return;
    }
    if inner.life.step(LifeEvent::Keeping(KeepingEvent::Asking)) != Some(crate::lifecycle::Out::Keeping(KeepingOut::Ask)) {
        return;
    }
    // A person's command under way on its own thread, whose change isn't in to be looked at yet, or a hand-off's: a try that didn't get the lock. No agent's
    // change can be under way: every one is refused while it tries, before it's counted (`answers_agents`).
    if busy(inner) {
        back(inner, reads, false, None);
        return;
    }
    let (verdict, heard) = std::sync::mpsc::channel();
    let asking = Arc::clone(inner);
    let sent = std::thread::Builder::new()
        .name("recto-record-lock".into())
        .stack_size(512 * 1024)
        .spawn(move || ask(&asking, verdict, &heard));
    if sent.is_err() {
        back(inner, reads, false, None);
    }
}

/// **A try at the record's lock, on a thread of its own:** the store's one `try_lock` (an open of the lock file, which a sleeping disk can make slow), told to the
/// session thread, which decides; with the lock, this thread then reads the saved session, or lets the lock go again, as it's told.
fn ask(inner: &Arc<Inner>, verdict: std::sync::mpsc::Sender<Verdict>, heard: &std::sync::mpsc::Receiver<Verdict>) {
    let got = inner.saver.store.try_to_keep();
    recto_platform::slow::pause(recto_platform::slow::Seam::RecordLock);
    inner.core.push(CoreCall::TryBack(TryBack { got, verdict }));
    if !got {
        return;
    }
    match heard.recv() {
        Ok(Verdict::Read) => launching::read_state(inner),
        Ok(Verdict::LetGo) => inner.saver.store.let_go(),
        Ok(Verdict::GiveBack) => inner.saver.store.give_back(),
        // The session thread ended without deciding: the app's going, and the process takes the lock with it.
        Err(_) => {}
    }
}

/// What the session thread decided about a try that came back with the lock.
#[derive(Debug)]
pub enum Verdict {
    /// Keep it: read the saved session, as a launch does.
    Read,
    /// The trying stopped while it was out: let it go again, writing nothing.
    LetGo,
    /// Not now (a person's command came under way while it was out): give it back, and try again on the next turn.
    GiveBack,
}

/// A try back from its thread: whether it got the lock, and where the decision about it goes.
#[derive(Debug)]
pub struct TryBack {
    got: bool,
    verdict: std::sync::mpsc::Sender<Verdict>,
}

#[cfg(test)]
impl TryBack {
    /// One that didn't get the lock, for a test of the queue's variants.
    pub(crate) fn for_test() -> Self {
        Self {
            got: false,
            verdict: std::sync::mpsc::channel().0,
        }
    }
}

/// **A try is back,** on the session thread: the window's looked at first, so whatever the person did while it was out counts before the lock does; then the try's
/// taken in, and the thread that made it told to read, or to let the lock go again.
pub(super) fn try_back(inner: &Arc<Inner>, reads: &FileReads, back_from: TryBack) {
    back(inner, reads, back_from.got, Some(&back_from.verdict));
}

fn back(inner: &Arc<Inner>, reads: &FileReads, got: bool, verdict: Option<&std::sync::mpsc::Sender<Verdict>>) {
    let mut given_back = false;
    if inner.life.keeping().trying() {
        let now_as = as_done(inner, reads);
        let _ = step(inner, KeepingEvent::Looked { now_as });
        given_back = got && inner.life.keeping().trying() && busy(inner);
    }
    let out = inner.life.step(LifeEvent::Keeping(KeepingEvent::Tried { got: got && !given_back }));
    let decided = match out {
        _ if given_back => Some(Verdict::GiveBack),
        Some(crate::lifecycle::Out::Keeping(KeepingOut::Restore)) => Some(Verdict::Read),
        Some(crate::lifecycle::Out::Keeping(KeepingOut::LetGo)) => Some(Verdict::LetGo),
        out => {
            let _ = settled(inner, out);
            None
        }
    };
    if let (Some(decided), Some(verdict)) = (decided, verdict) {
        // A thread that's gone has nothing to read or let go.
        let _ = verdict.send(decided);
    }
    inner.core.push(CoreCall::Reconcile);
}

/// Something under way that a late try's restore would replace or lose: a person's command on its own thread (its change isn't in to be looked at), or a
/// hand-off.
fn busy(inner: &Inner) -> bool {
    inner.life.persons_running.load(Ordering::SeqCst) > 0 || inner.host.handing_off.load(Ordering::SeqCst)
}

/// **Whether an agent's command is answered while keeping the record is settling** (trying or restoring, [`crate::lifecycle::keeping::Keeping::refuses_agents_changes`]):
/// only those that are purely a look at the state, which are the ones that only ask, but `session save`, which writes. Everything else is refused `busy` before
/// it's counted as under way, so no agent's change is ever under way while it tries: a page's text, a measure and a wait included, as each reads a tab that
/// the restore is about to replace. `qa drive` acts as the person, in a quiet run, through the methods the window calls, so what it does counts as the person's:
/// it can stop the trying as their own action would, which is what it's for.
pub(crate) fn answers_agents(command: &recto_agent::Command) -> bool {
    command.only_asks() && !matches!(command, recto_agent::Command::Record(recto_agent::commands::Record::SessionSave(_)))
}

/// **Whether a writer of the record may write now:** this copy holds the record's lock and keeps the record, from the launch or since a late try's saved session
/// is in. Not while it's trying (a try may be back with the lock and not yet decided on), restoring (what it would gather is the window before the restore), or
/// once it's settled not to keep it. Looked at by every writer before it takes the saver's gate, so one that can't write never waits for it, and again under it.
pub(crate) fn may_write(inner: &Inner) -> bool {
    inner.life.keeping().writes() && inner.saver.store.holds_the_record()
}

/// **A step of keeping the record, and what it settles carried out:** every step the session thread makes of it, so the one place keeping settles is where
/// the updater is told of a launch held back, and a launch held back and settled can't be left untold, by any route (a debug build says so if it is).
fn step(inner: &Arc<Inner>, event: KeepingEvent) -> Option<crate::lifecycle::Out> {
    let out = inner.life.step(LifeEvent::Keeping(event));
    settled(inner, out)
}

/// What keeping settled to, carried out: told to the updater either way; the notice when it isn't kept; and the host told to make the window again from a late
/// try's restore, which is returned for the caller to tell it once the session thread's call has ended.
fn settled(inner: &Arc<Inner>, out: Option<crate::lifecycle::Out>) -> Option<crate::lifecycle::Out> {
    let host = match out {
        Some(crate::lifecycle::Out::Keeping(KeepingOut::Kept { remake })) => {
            tell_the_updater(inner);
            remake.then_some(crate::lifecycle::Out::RestoreReady { with_state: true, late: true })
        }
        Some(crate::lifecycle::Out::Keeping(KeepingOut::SayNotKept)) => {
            say_not_kept(inner);
            None
        }
        _ => None,
    };
    debug_assert!(
        !(inner.life.keeping().settled() && lock(&inner.life.update_facts).untold()),
        "keeping settled ({:?}) with the launch still held back from the updater",
        inner.life.keeping()
    );
    host
}

/// The window won't be remembered: said as it always was, in the record's words, and the updater's told this is a second copy.
fn say_not_kept(inner: &Arc<Inner>) {
    *lock(&inner.host.notice) = Some(record::notices::OTHER_INSTANCE.to_owned());
    tell_the_updater(inner);
}

/// **The updater told of a launch that was held back,** now that keeping has settled. What's held is taken here, on the session thread, so it can't be left
/// held once settled; it's told on a thread of its own, as that touches files: of the launch, if it hadn't been (a second copy, or a copy that came to keep the
/// record by another route than a late try, whose read tells it), and then of a first frame shown meanwhile, with the tabs that were read. Nothing at all for a
/// launch that wasn't held back.
fn tell_the_updater(inner: &Arc<Inner>) {
    let (facts, held) = {
        let mut launch = lock(&inner.life.update_facts);
        (launch.facts.take(), launch.held)
    };
    if facts.is_none() && !held {
        return;
    }
    let facts = Arc::new(Mutex::new(facts));
    let (telling, theirs) = (Arc::clone(inner), Arc::clone(&facts));
    let told = std::thread::Builder::new()
        .name("recto-update-frame".into())
        .stack_size(512 * 1024)
        .spawn(move || tell(&telling, lock(&theirs).take()));
    // With no thread to be had, what's held isn't left held for the rest of the run: told here, on the session thread, as the one thing that can still tell it.
    if told.is_err() {
        tell(inner, lock(&facts).take());
    }
}

/// Told, on the thread that tells it.
fn tell(inner: &Inner, facts: Option<recto_update::updater::Facts>) {
    if let Some(facts) = facts
        && inner.updates.launched(facts) == recto_update::updater::AtLaunch::Exit
    {
        std::process::exit(0);
    }
    updates::first_frame_after_launch(inner);
}

/// **The window as it is, kept until it's made again** from a late keep's restore (`apply_restored`): its frame, focus, sidebar width and Settings, which the
/// restore put in `from_state` for the window made again ([`Inner::window_replaced`]), so if the person touches it first and it isn't made again, the record says
/// what the real window is.
pub(super) fn window_as_it_is(app: &mut AppState, real: &AppState) {
    app.window.clone_from(&real.window);
    app.focus = real.focus;
    app.sidebar_width = real.sidebar_width;
    app.settings_window = real.settings_window;
}
