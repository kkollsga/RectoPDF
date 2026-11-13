//! **The record's lock come late, carried out on the session thread:** what [`crate::lifecycle::keeping`] decides (a launch that didn't get the lock tries
//! again quietly for ten seconds), done. Every try is the store's one `try_lock` ([`recto_control::record_store::RecordStore::try_to_keep`]) on the session
//! thread's own deadline, with nothing held across it; the UI thread never waits for any of it.

use super::*;
use crate::lifecycle::Event as LifeEvent;
use crate::lifecycle::keeping::{Event as KeepingEvent, Out as KeepingOut};
use std::hash::{Hash as _, Hasher as _};

/// **What the window has now, as one number:** the session as it would be recorded (its tabs, places, view, Find, recent files) and the look a person chose. So
/// anything a person does that a restore would replace changes it, and what the host sets by itself (the window's frame, focus, the contents' scroll) doesn't.
fn as_done(inner: &Inner) -> u64 {
    let record = inner.host.session.with_session(|session| session.to_record());
    let appearance = lock(&inner.window.app).appearance;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_vec(&(record, appearance)).unwrap_or_default().hash(&mut hasher);
    hasher.finish()
}

/// **The state's been taken in** (`session::apply_restored`), with `other_instance` when it was refused for the lock: the first time, whether this copy keeps
/// the record starts here. Says whether the notice that came with what was read is said now: not while it's trying.
pub(super) fn taken_in(inner: &Inner, other_instance: bool, first_time: bool) -> bool {
    if first_time {
        let keeps = inner.saver.store.holds_the_record();
        let as_read = if keeps { 0 } else { as_done(inner) };
        let _ = inner.life.step(LifeEvent::Keeping(KeepingEvent::Read { keeps, as_read }));
    }
    !(other_instance && inner.life.keeping().trying())
}

/// After every call on the session thread, while it's trying: the person having opened or changed something stops it, and that's said now.
pub(super) fn look_after_call(inner: &Inner) {
    if !inner.life.keeping().trying() {
        return;
    }
    let now_as = as_done(inner);
    if let Some(out) = inner.life.step(LifeEvent::Keeping(KeepingEvent::Looked { now_as })) {
        say(inner, out);
    }
}

/// **The session thread's deadline while it's trying:** a look at the window, then one try at the lock. When it came, the saved session is read now and taken in
/// as a launch's is, in this same turn, so nothing the person does can come between the look and the restore.
#[expect(
    clippy::vec_box,
    reason = "the session thread's own list of what waited for the state, as `apply_restored` takes it"
)]
pub(super) fn when_due(inner: &Arc<Inner>, synced: &mut Synced, reads: &mut FileReads, deferred: &mut Vec<Box<crate::queue::PersonCommand>>) {
    if !inner.life.keeping_due().is_some_and(|wait| wait.is_zero()) {
        return;
    }
    let now_as = as_done(inner);
    let looked = inner.life.step(LifeEvent::Keeping(KeepingEvent::Looked { now_as }));
    let out = if looked.is_some() {
        looked
    } else {
        let got = inner.saver.store.try_to_keep();
        inner.life.step(LifeEvent::Keeping(KeepingEvent::Tried { got }))
    };
    match out {
        Some(crate::lifecycle::Out::Keeping(KeepingOut::Restore)) => {
            // On the session thread, as `session restore` reads it: the lock's held, so the store doesn't wait for it.
            let loaded = updates::fall_back_if_unreadable(inner, inner.saver.store.load());
            updates::note_tabs_read(&loaded);
            let ready = apply_restored(inner, loaded, synced, reads, deferred);
            after_call(inner, synced, reads, Vec::new());
            inner.carry_out(ready);
        }
        Some(out) => {
            say(inner, out);
            inner.core.push(CoreCall::Reconcile);
        }
        None => {}
    }
}

/// The window won't be remembered: said as it always was, in the record's words.
fn say(inner: &Inner, out: crate::lifecycle::Out) {
    if out == crate::lifecycle::Out::Keeping(KeepingOut::SayNotKept) {
        *lock(&inner.host.notice) = Some(record::notices::OTHER_INSTANCE.to_owned());
    }
}
