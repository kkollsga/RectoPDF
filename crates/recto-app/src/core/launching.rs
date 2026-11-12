//! **The launch, as the host steps through it:** the state read on a thread of its own, the patience for it, and the window being up.

use super::*;

impl Inner {
    /// The window is up: files named at launch start opening. The host's own part (a quiet run's line, the print sweep) is the host's.
    pub fn ready(&self, open: &[PathBuf]) {
        for path in open {
            self.core.push(CoreCall::Open(path.clone()));
        }
        self.core.push(CoreCall::Reconcile);
    }

    /// Reads the state, on a thread of its own, and hears `restore_ready` when it's applied. It's what the window waits for, up to `restore_patience_seconds`: its frame and the place on it come from what's read.
    pub fn begin_restore(self: &Arc<Self>) {
        let inner = Arc::clone(self);
        let spawned = std::thread::Builder::new().name("recto-restore".into()).stack_size(1024 * 1024).spawn(move || {
            // Before the record: a version that keeps failing goes back here, and not by reading on.
            let facts = lock(&inner.life.update_facts).take();
            let misreads = updates::misreads_the_record(facts.as_ref());
            if let Some(facts) = facts
                && inner.updates.launched(facts) == recto_update::updater::AtLaunch::Exit
            {
                std::process::exit(0);
            }
            recto_platform::slow::pause(recto_platform::slow::Seam::State);
            let loaded = if misreads { Loaded::Nothing } else { inner.saver.store.load() };
            let loaded = updates::fall_back_if_unreadable(&inner, loaded);
            updates::note_tabs_read(&loaded);
            inner.core.push(CoreCall::Restored(Box::new(loaded)));
        });
        if spawned.is_err() {
            self.core.push(CoreCall::Restored(Box::new(Loaded::Nothing)));
        }
    }

    /// The state hasn't been read in time, or a restored tab's file isn't answering. What's on the screen goes on without it.
    pub fn restore_patience_over(&self) {
        self.core.push(CoreCall::RestorePatienceOver);
    }

    /// Whether the state's been read and taken in. A window that went up without it, past the patience, is made again from it once it has: see the host's `restoreReady`.
    pub fn state_read(&self) -> bool {
        self.life.restored.load(Ordering::SeqCst)
    }
}
