//! **The page-identity thread** (the page matcher): reads what the pages an edit names are, while their file is still the version the edits are about,
//! so the edits can follow their pages if the file changes on disk (`recto_session::identify`). One for the app, started when an edit names pages not
//! yet read and gone when there are none: no thread at rest, none per tab (rule 1). It takes the session's lock only to ask what to read and to hand in
//! what it read, never while it reads a file, so the UI and the session thread wait for it nowhere (rule 3). It counts in what's running
//! ([`super::running::Running::identifying`]), so a settle, a hand-off and the QA waits see it; a quit doesn't wait for it, as what it reads is read
//! again after a restart from the pages the journal names, which a restore looks for at once ([`after_restore`]). Its jobs are bounded by bytes
//! (`recto_session::identify::MAX_JOB_BYTES`), not time, and it keeps the file it's reading open across them, so a file's page map is built once.

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
            // Edits that may follow their pages into another version of their file: a look at each list's own field, no tab looked up.
            (journal_work && recto_session::identify::has_work(session)) || recto_session::version_change::wanted(session),
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
        // The file being read, kept across jobs while it's the same version, and gone with the thread.
        let mut opened = None;
        loop {
            self.again.store(false, Ordering::SeqCst);
            let jobs = host.session.with_session(|session| recto_session::identify::work(session));
            let mut taken = false;
            for job in &jobs {
                // Off the lock: a file on a slow disk holds only this thread.
                let Some(found) = recto_session::identify::read_in(job, &mut opened) else {
                    continue;
                };
                taken |= host.session.with_session(|session| recto_session::identify::identified(session, &found));
            }
            if taken {
                // What was read goes in the journals, as an edit's operations do.
                host.saver.edit_made();
            }
            // Edits whose file changed under them, following their pages into the new version or held (`recto_session::version_change`).
            let follows = host.session.with_session(recto_session::version_change::work);
            for job in &follows {
                // Off the lock, as a page's identity is read.
                let matched = recto_session::version_change::read(job);
                let followed = host.session.with_session(|session| match &matched {
                    Some(matched) => recto_session::version_change::followed(session, matched),
                    None => {
                        recto_session::version_change::not_read(session, job);
                        None
                    }
                });
                if let Some(followed) = followed {
                    host.saver.edit_made();
                    host.core.push(crate::queue::CoreCall::Reconcile);
                    host.tell(vec![recto_agent::event::edits_followed(&followed)]);
                }
            }
            // Each job followed, held, was marked unreadable or was overtaken by an edit, whose own job comes next: each round is progress.
            let taken = taken || !follows.is_empty();
            // More to read (a job read only to its byte budget), or asked again meanwhile: look again. A file that keeps refusing gives no work back,
            // and one with a page of no identity is given up on, so this ends once every job's had its turn and nothing new is asked.
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

/// **A restore brought back lists naming pages not yet read** (a quit while they were being read): they're read now, while their files are, it's
/// hoped, still the versions their edits are about, without waiting for an edit to ask. A look at the edited tabs' lists, once per restore.
pub(crate) fn after_restore(host: &Arc<AppHost>, ids: &Arc<PageIds>) {
    if host.session.with_session(|session| recto_session::identify::has_work(session)) {
        ids.wanted(host);
    }
}

/// **What QA reads of the page matcher's reading, as counts only** (a digest is of a document's content, and the report is read by an agent): pages the
/// edited tabs name and how many are read, whether the thread is reading, and whether the journals and their sidecars have everything (`caught_up`),
/// so a test waits for that rather than a time.
pub(crate) fn report(host: &AppHost, ids: &PageIds) -> serde_json::Value {
    let ((named, read), caught_up) = host
        .session
        .with_session(|session| (recto_session::identify::counts(session), !session.journal_look().has_work()));
    serde_json::json!({ "named": named, "read": read, "reading": ids.busy(), "caught_up": caught_up })
}
