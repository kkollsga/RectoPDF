//! **A session as its host holds it** (R3.5b): its lock and the confinement cell the host reads with no lock, together, so the session can't be reached
//! out of the lock by a way that doesn't put the host's cell back after. The host's cell is the authority, as what it reads with no lock (the executor's
//! `quiet_files`, a new file's check, the installer, as-person) must be what the session goes by; a step that replaced the whole session
//! (`*session = Session::new(..)`, or a swap with another host's) brought a cell of its own, and the guard's drop puts the host's back, never widening the
//! session ([`registry::Tabs::keep_confinement`](crate::registry::Tabs)). Only this does that, so nothing outside the crate can put a session on a cell
//! of its choosing.
//!
//! It's the host's one mutex, moved here: no lock is added and there's no new order to take them in.

use crate::Session;
use crate::confined::Confinement;
use std::sync::{Mutex, MutexGuard, PoisonError, TryLockError};

/// A session, its lock and its host's confinement cell. Built once, around a session given by value, so there's never a second one for a session.
pub struct Held<L> {
    session: Mutex<Session<L>>,
    /// The session's own cell when it was given, shared ([`Confinement::shared`]): the host's, read with no lock.
    cell: Confinement,
}

impl<L> Held<L> {
    /// Holds the session, its cell as it is now being the host's for good.
    pub fn new(session: Session<L>) -> Self {
        let cell = session.tabs.confinement().shared();
        Self {
            session: Mutex::new(session),
            cell,
        }
    }

    /// The host's cell, to read with no lock: whether the session is confined, and to what.
    pub fn confinement(&self) -> &Confinement {
        &self.cell
    }

    /// **The session, once its lock is free.** A lock poisoned by a panic in an earlier step is taken all the same, as the host always has: the session's
    /// state is still whole, as no step leaves it half-changed on a panic that matters, and refusing every later command would lose the person's edits.
    pub fn lock(&self) -> HeldGuard<'_, L> {
        HeldGuard {
            session: self.session.lock().unwrap_or_else(PoisonError::into_inner),
            cell: &self.cell,
        }
    }

    /// The session if its lock is free now, for a thread that mustn't wait (the UI thread). Poisoned, as [`Self::lock`].
    pub fn try_lock(&self) -> Option<HeldGuard<'_, L>> {
        let session = match self.session.try_lock() {
            Ok(session) => session,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return None,
        };
        Some(HeldGuard { session, cell: &self.cell })
    }
}

/// The session in hand, under its lock. Dropping it puts the host's cell back if the step swapped another session in.
pub struct HeldGuard<'a, L> {
    session: MutexGuard<'a, Session<L>>,
    cell: &'a Confinement,
}

impl<L> std::ops::Deref for HeldGuard<'_, L> {
    type Target = Session<L>;

    fn deref(&self) -> &Session<L> {
        &self.session
    }
}

impl<L> std::ops::DerefMut for HeldGuard<'_, L> {
    fn deref_mut(&mut self) -> &mut Session<L> {
        &mut self.session
    }
}

/// An `Arc::ptr_eq` when nothing was replaced, under the lock the step held. No shipping code replaces a session, so one that did is a loud failure in a
/// debug build, and quietly mended in a release one; never while a panic is already unwinding, which would abort.
impl<L> Drop for HeldGuard<'_, L> {
    fn drop(&mut self) {
        let kept = self.session.tabs.keep_confinement(self.cell);
        if !std::thread::panicking() {
            debug_assert!(
                kept,
                "a step replaced the host's session, and its confinement cell with it: make a session with its host, never inside its lock"
            );
        }
    }
}
