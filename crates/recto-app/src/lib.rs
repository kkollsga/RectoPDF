//! **The app's own logic with no toolkit and no bridge:** the view models the window shows, projected from the session and a few plain facts ([`model`]), and the window's features'
//! rules and state, each in its module, taking what they need as plain parameters, a small handle, or a trait this crate owns ([`clock::Clock`]). `recto-ffi` converts and forwards.

pub mod chrome;
pub mod clock;
pub mod linking;
pub mod menus;
pub mod model;
pub mod palette;
pub mod passwords;
pub mod privacy;
pub mod prompts;
pub mod sidebar_access;
pub mod snapshot_menu;
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;

pub use model::*;

use std::sync::{Mutex, MutexGuard, PoisonError};

/// A lock that a panic elsewhere doesn't poison, as everywhere in the app.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
