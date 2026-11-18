//! The host acknowledges a print without doing disk work. One short-lived worker owns publication and descriptor close; failed starts remain pending.

use super::*;
use crate::queue::WayBack;
use std::time::Duration;

const RETRY_MS: u64 = 1000;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Key {
    pub path: PathBuf,
    pub serial: u64,
}

pub(super) enum Status {
    Host,
    Pending(Context),
    Running,
}

pub(super) struct Context {
    core: Arc<CoreQueue>,
    way_back: Arc<WayBack>,
}

pub(super) struct Entry {
    pub key: Key,
    pub file: Option<File>,
    pub status: Status,
}

#[derive(Default)]
pub(super) struct Handing {
    pub entries: Vec<Entry>,
    worker: bool,
    retry_at: Option<u64>,
}

impl Handing {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn pending(&self) -> bool {
        self.entries.iter().any(|entry| matches!(entry.status, Status::Pending(_)))
    }

    fn due(&self, now: u64) -> Option<Duration> {
        (!self.worker && self.pending()).then(|| Duration::from_millis(self.retry_at.unwrap_or(now).saturating_sub(now)))
    }
}

/// The exact handed key claims completion once. This path only changes bounded state and wakes the session; it never starts a thread or touches disk.
pub fn request(printing: &Printing, core: &Arc<CoreQueue>, path: &str, serial: u64) {
    let claimed = {
        let mut handing = lock(&printing.handing);
        if let Some(entry) = handing
            .entries
            .iter_mut()
            .find(|entry| entry.key.serial == serial && entry.key.path.as_os_str() == path && matches!(entry.status, Status::Host))
        {
            entry.status = Status::Pending(Context {
                core: Arc::clone(core),
                way_back: Arc::new(core.way_back()),
            });
            true
        } else {
            false
        }
    };
    if claimed {
        core.nudge();
    }
}

impl Printing {
    /// Only the completion phase, not the printer's lifetime, counts in the bounded quit grace.
    pub fn publishing(&self) -> bool {
        lock(&self.handing).entries.iter().any(|entry| !matches!(entry.status, Status::Host))
    }

    /// Pure deadline projection; the session may ask this while holding its queue mutex.
    pub(crate) fn completion_due(&self, now: u64) -> Option<Duration> {
        lock(&self.handing).due(now)
    }
}

/// Session-thread dispatch outside its queue mutex. A failed spawn owns no sole resource: every file, target and way-back remains pending in shared state.
pub(crate) fn when_due(printing: Arc<Printing>, core: &Arc<CoreQueue>, now: u64) {
    {
        let mut handing = lock(&printing.handing);
        if !handing.due(now).is_some_and(|wait| wait.is_zero()) {
            return;
        }
        handing.worker = true;
        handing.retry_at = None;
    }
    #[cfg(any(test, feature = "testkit"))]
    let refused = printing
        .hooks
        .refused_spawns
        .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| left.checked_sub(1))
        .is_ok();
    #[cfg(not(any(test, feature = "testkit")))]
    let refused = false;
    let worker = Arc::clone(&printing);
    let way_back = Arc::clone(core);
    let spawned = if refused {
        Err(std::io::Error::other("publication worker refused for a test"))
    } else {
        std::thread::Builder::new()
            .name("recto-print-finish".into())
            .stack_size(512 * 1024)
            .spawn(move || {
                #[cfg(any(test, feature = "testkit"))]
                worker.hooks.starts.fetch_add(1, Ordering::SeqCst);
                let mut working = Working {
                    printing: &worker,
                    core: way_back,
                    last_way_back: None,
                };
                loop {
                    let job = {
                        let mut handing = lock(&worker.handing);
                        let at = handing.entries.iter().position(|entry| matches!(entry.status, Status::Pending(_)));
                        at.and_then(|at| Job::take(&worker, &mut handing.entries[at]))
                    };
                    let Some(mut job) = job else { return };
                    working.last_way_back = job.context.as_ref().map(|context| Arc::clone(&context.way_back));
                    job.publish();
                }
            })
    };
    if spawned.is_err() {
        let mut handing = lock(&printing.handing);
        handing.worker = false;
        handing.retry_at = Some(now.saturating_add(RETRY_MS));
        drop(handing);
        core.nudge();
    }
}

/// Clears the worker slot even after a panic, so other pending entries can be dispatched rather than kept forever.
struct Working<'a> {
    printing: &'a Printing,
    core: Arc<CoreQueue>,
    last_way_back: Option<Arc<WayBack>>,
}
impl Drop for Working<'_> {
    fn drop(&mut self) {
        lock(&self.printing.handing).worker = false;
        self.core.nudge();
    }
}

/// One owned job. Its entry stays marked Running through publication, staging cleanup and the descriptor's close outside all UI-taken mutexes.
struct Job<'a> {
    printing: &'a Printing,
    key: Key,
    file: Option<File>,
    context: Option<Context>,
    target: Option<(PathBuf, PathBuf)>,
}

impl<'a> Job<'a> {
    fn take(printing: &'a Printing, entry: &mut Entry) -> Option<Self> {
        if matches!(entry.status, Status::Running) {
            return None;
        }
        let file = entry.file.take()?;
        let context = match std::mem::replace(&mut entry.status, Status::Running) {
            Status::Pending(context) => Some(context),
            _ => None,
        };
        Some(Self {
            printing,
            key: entry.key.clone(),
            file: Some(file),
            context,
            target: None,
        })
    }

    fn publish(&mut self) {
        // This statement releases qa_target's guard before any IO or test hold.
        self.target = lock(&self.printing.qa_target).take();
        #[cfg(test)]
        hold_handoff(&super::BEFORE_PUBLICATION);
        if let Some((written, out)) = &self.target {
            #[cfg(any(test, feature = "testkit"))]
            self.printing.hooks.before_publish();
            if written.exists() && recto_agent::publish_picture(written, out).is_err() {
                self.printing
                    .say("The print couldn't be put there: a file is already there, and it won't be replaced.", false);
            }
        }
    }
}

impl Drop for Job<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.printing.say(person::PANICKED, false);
        }
        if let Some((written, _)) = self.target.take() {
            let _ = std::fs::remove_file(written);
        }
        drop(self.file.take());
        lock(&self.printing.handing).entries.retain(|entry| entry.key != self.key);
        if let Some(context) = self.context.take() {
            context.core.push(CoreCall::Reconcile);
            // Only after the final word is queued; includes the pending period before a successful worker start.
            drop(context);
        }
    }
}

#[cfg(test)]
pub(super) fn finish_for_test(printing: &Printing, path: &str) {
    let job = {
        let mut handing = lock(&printing.handing);
        let at = handing.entries.iter().position(|entry| entry.key.path.as_os_str() == path);
        at.and_then(|at| Job::take(printing, &mut handing.entries[at]))
    };
    if let Some(mut job) = job {
        job.publish();
    }
}

#[cfg(test)]
mod tests;
