//! **The record's lock come late, carried out:** what [`crate::lifecycle::keeping`] decides (a launch that didn't get the lock tries again quietly for ten
//! seconds), done. Every try is the store's one `try_lock` ([`recto_control::record_store::RecordStore::try_to_keep`]) on the session thread's own deadline,
//! with nothing held across it; the UI thread never waits for any of it.
//!
//! **The order, once a try gets the lock** (the review of 2026-11-13's two blockers):
//! 1. The try is made under the saver's gate and the life's lock, so no writer of the record is between gathering what it writes and writing it, and no quit or
//!    hand-off begins between the look at whether a try is due and the try; and with no agent's change under way (one that is makes it a try that didn't get the
//!    lock). The machine is then restoring, and from here until the saved session is in, every writer writes nothing (`saver`) and what the person does waits,
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
/// the notice that came with what was read is said now (not while it's trying), and what the app's life comes to: the window made again from a late keep's
/// restore.
pub(super) fn taken_in(inner: &Arc<Inner>, other_instance: bool, first_time: bool, reads: &FileReads) -> (bool, Option<crate::lifecycle::Out>) {
    let keeps = inner.saver.store.holds_the_record();
    let mut out = None;
    if first_time || keeps {
        let as_read = if keeps { 0 } else { as_done(inner, reads) };
        out = inner.life.step(LifeEvent::Keeping(KeepingEvent::Read { keeps, as_read }));
    }
    if matches!(out, Some(crate::lifecycle::Out::RestoreReady { .. })) {
        tell_the_updater(inner, false);
    }
    (!(other_instance && inner.life.keeping().trying()), out)
}

/// After every call on the session thread, while it's trying: the person having opened or changed something stops it, and that's said now.
pub(super) fn look_after_call(inner: &Arc<Inner>, reads: &FileReads) {
    if !inner.life.keeping().trying() {
        return;
    }
    let now_as = as_done(inner, reads);
    if let Some(out) = inner.life.step(LifeEvent::Keeping(KeepingEvent::Looked { now_as })) {
        say(inner, out);
    }
}

/// **The session thread's deadline while it's trying:** a look at the window, then one try at the lock (see the module's order).
pub(super) fn when_due(inner: &Arc<Inner>, reads: &FileReads) {
    if !inner.life.keeping_due().is_some_and(|wait| wait.is_zero()) {
        return;
    }
    let now_as = as_done(inner, reads);
    if let Some(out) = inner.life.step(LifeEvent::Keeping(KeepingEvent::Looked { now_as })) {
        say(inner, out);
        inner.core.push(CoreCall::Reconcile);
        return;
    }
    let out = {
        let _gate = lock(&inner.saver.gate);
        inner.life.try_keeping(
            || inner.host.running.load(Ordering::SeqCst) > 0 || inner.host.handing_off.load(Ordering::SeqCst),
            || inner.saver.store.try_to_keep(),
        )
    };
    match out {
        Some(crate::lifecycle::Out::Keeping(KeepingOut::Restore)) => {
            let reading = Arc::clone(inner);
            let spawned = std::thread::Builder::new()
                .name("recto-restore".into())
                .stack_size(1024 * 1024)
                .spawn(move || launching::read_state(&reading));
            if spawned.is_err() {
                launching::read_state(inner);
            }
        }
        Some(out) => {
            say(inner, out);
            inner.core.push(CoreCall::Reconcile);
        }
        None => {}
    }
}

/// The window won't be remembered: said as it always was, in the record's words, and the updater's told this is a second copy.
fn say(inner: &Arc<Inner>, out: crate::lifecycle::Out) {
    if out == crate::lifecycle::Out::Keeping(KeepingOut::SayNotKept) {
        *lock(&inner.host.notice) = Some(record::notices::OTHER_INSTANCE.to_owned());
        tell_the_updater(inner, true);
    }
}

/// The updater told of a launch that was held back, on a thread of its own as it touches files: as a second copy (`launch`), or, once a late keep's restore is
/// in (told of the launch by then, before the read), the first frame shown meanwhile with the tabs that were read.
fn tell_the_updater(inner: &Arc<Inner>, launch: bool) {
    let telling = Arc::clone(inner);
    let _ = std::thread::Builder::new()
        .name("recto-update-frame".into())
        .stack_size(512 * 1024)
        .spawn(move || {
            if launch {
                let _ = updates::tell_launch(&telling);
            }
            updates::first_frame_after_launch(&telling);
        });
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
