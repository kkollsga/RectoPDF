//! **The page-identity thread** (the page matcher): reads what the pages an edit names are, while their file is still the version the edits are about,
//! so the edits can follow their pages if the file changes on disk (`recto_session::identify`). One for the app, started when an edit names pages not
//! yet read and gone when there are none: no thread at rest, none per tab (rule 1). It takes the session's lock only to ask what to read and to hand in
//! what it read, never while it reads a file, so the UI and the session thread wait for it nowhere (rule 3). It counts in what's running
//! ([`super::running::Running::identifying`]), so a settle, a hand-off and the QA waits see it; a quit doesn't wait for it, as what it reads is read
//! again after a restart from the pages the journal names.

use super::host::AppHost;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The thread's state: whether it's there, and whether it's been asked again since it last looked.
#[derive(Debug, Default)]
pub(crate) struct PageIds {
    running: AtomicBool,
    again: AtomicBool,
}

/// **What a call left for the journals** (a journal retired; operations waiting), read under the one lock the session thread takes for it after each
/// call, and the page-identity thread asked to read where an edit named pages not read: a look at the edited tabs' lists, only when there's journal work.
pub(crate) fn journal_work_after_a_call(host: &Arc<AppHost>, ids: &Arc<PageIds>) -> (bool, bool) {
    let (retired, journal_work, wanted) = host.session.with_session(|session| {
        let journal_work = session.journal_look().has_work();
        (
            session.journals().has_pending(),
            journal_work,
            journal_work && recto_session::identify::has_work(session),
        )
    });
    if wanted {
        ids.wanted(host);
    }
    (retired, journal_work)
}

impl PageIds {
    /// Whether the thread is there now.
    pub(crate) fn busy(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// **Pages are named and not read:** the thread looks, started if it isn't there. On the session thread, after an edit; an atomic or two when
    /// it's running already.
    pub(crate) fn wanted(self: &Arc<Self>, host: &Arc<AppHost>) {
        self.again.store(true, Ordering::SeqCst);
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let (ids, host) = (Arc::clone(self), Arc::clone(host));
        let spawned = std::thread::Builder::new().name("recto-page-ids".into()).stack_size(512 * 1024).spawn(move || {
            ids.run(&host);
            host.core.nudge();
        });
        if spawned.is_err() {
            // Nothing's lost: the pages stay named, and the next edit asks again.
            self.running.store(false, Ordering::SeqCst);
        }
    }

    fn run(&self, host: &AppHost) {
        loop {
            self.again.store(false, Ordering::SeqCst);
            let jobs = host.session.with_session(|session| recto_session::identify::work(session));
            let mut taken = false;
            for job in &jobs {
                // Off the lock: a file on a slow disk holds only this thread.
                let Some(found) = recto_session::identify::read(job) else { continue };
                taken |= host.session.with_session(|session| recto_session::identify::identified(session, &found));
            }
            if taken {
                // What was read goes in the journals, as an edit's operations do.
                host.saver.edit_made();
            }
            // More to read (a job read only its first pages), or asked again meanwhile: look again. A file that keeps refusing gives no work back, so
            // this ends once every job's had its turn and nothing new is asked.
            if taken || self.again.load(Ordering::SeqCst) {
                continue;
            }
            self.running.store(false, Ordering::SeqCst);
            // An ask between the look and the store saw the thread there and went: take it up, unless another thread has.
            if !self.again.load(Ordering::SeqCst) || self.running.swap(true, Ordering::SeqCst) {
                return;
            }
        }
    }
}

/// **A list's record as QA reads it, with what its pages are said only as counts:** a digest is of a document's content, and `qa state` is read by an
/// agent (at Full, in a development build), so the named pages are how many, how many read, and whether some couldn't be.
pub(crate) fn without_digests(mut list: serde_json::Value) -> serde_json::Value {
    if let Some(ids) = list.get_mut("identities") {
        let pages = ids["pages"].as_array().map_or(0, Vec::len);
        let read = ids["pages"].as_array().map_or(0, |pages| pages.iter().filter(|page| page.is_array()).count());
        *ids = serde_json::json!({ "pages": pages, "read": read, "over_cap": ids["over_cap"] });
    }
    list
}
