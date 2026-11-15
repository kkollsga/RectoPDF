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
        // The file being read, kept across jobs while it's the same version, and gone with the thread; and a follow being read, the same.
        let mut opened = None;
        let mut following: Option<recto_session::version_change::Following> = None;
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
            // Edits whose file changed under them, following their pages into the new version or held (`recto_session::version_change`): one follow
            // at a time, a step a round (bounded by bytes and pages, as a job above is), so this thread is never held by one long file in one piece.
            let stepped = self.follow_step(host, &mut following);
            let taken = taken || stepped;
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

impl PageIds {
    /// **One step of a follow:** the one being read goes on if it's still wanted (else it's dropped, and the next job is about the list as it is now),
    /// or the next tab's starts. Its reading is off the lock; what it found is taken in under it. Says whether there was a step.
    fn follow_step(&self, host: &AppHost, following: &mut Option<recto_session::version_change::Following>) -> bool {
        use recto_session::version_change::{self, Read};
        if following
            .as_ref()
            .is_some_and(|follow| !host.session.with_session(|session| version_change::current(session, follow.job())))
        {
            *following = None;
        }
        if following.is_none() {
            let (wanted, job) = host
                .session
                .with_session(|session| (version_change::wanted(session), version_change::work(session).into_iter().next()));
            let Some(job) = job else {
                // A list wanted and given no job was marked tried without a read (protected meanwhile, past its cap): it no longer says it's being
                // looked at, so the tab's words are made again. Only then, so an edit's own reading costs no extra publish.
                if wanted {
                    host.core.push(crate::queue::CoreCall::Reconcile);
                }
                return false;
            };
            // Off the lock: the open, and the descriptor's own check.
            match version_change::start(job.clone()) {
                Some(follow) => *following = Some(follow),
                None => {
                    host.session.with_session(|session| version_change::not_read(session, &job));
                    host.core.push(crate::queue::CoreCall::Reconcile);
                    return true;
                }
            }
        }
        let Some(follow) = following.as_mut() else { return false };
        match version_change::read_part(follow) {
            Read::More => {}
            Read::NotRead => {
                let job = follow.job().clone();
                *following = None;
                host.session.with_session(|session| version_change::not_read(session, &job));
                // Held now: what the tab says changes from "being looked at" to what's so.
                host.core.push(crate::queue::CoreCall::Reconcile);
            }
            Read::Matched(matched) => {
                *following = None;
                if let Some(followed) = host.session.with_session(|session| version_change::followed(session, &matched)) {
                    host.saver.edit_made();
                    host.tell(vec![recto_agent::event::edits_followed(&followed)]);
                }
                // Followed or held, what the tab says changes from "being looked at" to what's so.
                host.core.push(crate::queue::CoreCall::Reconcile);
            }
        }
        true
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
