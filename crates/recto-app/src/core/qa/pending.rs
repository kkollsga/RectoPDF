//! The watchdog label belongs to the most recently admitted host ask still pending.

use super::{QaBridge, lock};
use crate::watchdog::Label;
use std::sync::atomic::Ordering;

enum Reply {
    Pending,
    Delivered(Result<String, String>),
    Consumed,
}

/// One fixed label beside the existing answer slot, bounded by the server's eight active jobs. A consumed slot retains attribution through decoding without retaining its payload or accepting another reply.
pub(super) struct PendingAnswer {
    verb: Label,
    reply: Reply,
}

impl PendingAnswer {
    /// Returns an unowned/duplicate payload to be dropped after the map gate is released.
    pub(super) fn deliver(&mut self, answer: Result<String, String>) -> Option<Result<String, String>> {
        if matches!(self.reply, Reply::Pending) {
            self.reply = Reply::Delivered(answer);
            None
        } else {
            Some(answer)
        }
    }

    pub(super) fn take(&mut self) -> Option<Result<String, String>> {
        match std::mem::replace(&mut self.reply, Reply::Consumed) {
            Reply::Delivered(answer) => Some(answer),
            Reply::Pending => {
                self.reply = Reply::Pending;
                None
            }
            Reply::Consumed => None,
        }
    }
}

/// Owns just this ask's admission until it returns, including host errors, timeouts, and unwinding a listener callback.
pub(super) struct PendingVerb<'a> {
    qa: &'a QaBridge,
    id: u64,
}

impl<'a> PendingVerb<'a> {
    pub(super) fn admit(qa: &'a QaBridge, verb: &str) -> Self {
        let mut answers = lock(&qa.answers);
        // Allocate the id inside the admission gate: its order is admission order, not which caller obtained a ticket first.
        let id = qa.next_id.fetch_add(1, Ordering::Relaxed);
        let label = Label::new(verb);
        answers.insert(
            id,
            PendingAnswer {
                verb: label,
                reply: Reply::Pending,
            },
        );
        qa.watchdog.note_verb(Some(label.as_str()));
        Self { qa, id }
    }

    pub(super) fn id(&self) -> u64 {
        self.id
    }
}

impl Drop for PendingVerb<'_> {
    fn drop(&mut self) {
        let removed = {
            let mut answers = lock(&self.qa.answers);
            let removed = answers.remove(&self.id);
            let latest = answers.iter().max_by_key(|(id, _)| *id).map(|(_, slot)| slot.verb);
            self.qa.watchdog.note_verb(latest.as_ref().map(Label::as_str));
            removed
        };
        // Normally consumed/fixed metadata only; on unwind any delivered payload also drops outside the map gate.
        drop(removed);
    }
}
