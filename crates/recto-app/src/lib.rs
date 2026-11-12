//! **The app's own logic with no toolkit and no bridge:** the view models the window shows, projected from the session and a few plain facts ([`model`]), and the window's features'
//! rules and state, each in its module, taking what they need as plain parameters, a small handle, or a trait this crate owns ([`clock::Clock`], [`Listener`]). `recto-ffi` converts and forwards.

pub mod chrome;
pub mod clock;
pub mod find;
pub mod generated_contents;
pub mod hand_off;
pub mod launch;
pub mod linking;
pub mod listener;
pub mod menus;
pub mod model;
pub mod palette;
pub mod panes;
pub mod passwords;
pub mod pressure;
pub mod privacy;
pub mod prompts;
pub mod queue;
pub mod reader_view;
pub mod saver;
pub mod saving;
pub mod sidebar_access;
pub mod sleep_queue;
pub mod sleeper;
pub mod snapshot_menu;
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;
pub mod watchdog;

pub use listener::Listener;
pub use model::*;

use std::sync::{Mutex, MutexGuard, PoisonError};

/// A lock that a panic elsewhere doesn't poison, as everywhere in the app.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
