//! **The password prompt's record,** as the host draws it. Who's asked, what it says and how many tries are `recto_app::passwords`'s.

use recto_app::passwords as app;
pub(crate) use recto_app::passwords::{CANT_KEEP, Target, Wrong, stopped_asking};

/// The sheet's words, and which ask they're for.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PasswordPrompt {
    /// Handed back with the answer, so it goes to this file and no other.
    pub id: u64,
    pub title: String,
    pub message: String,
    pub placeholder: String,
    pub open_title: String,
    pub cancel_title: String,
}

impl From<app::PasswordPrompt> for PasswordPrompt {
    fn from(prompt: app::PasswordPrompt) -> Self {
        Self {
            id: prompt.id,
            title: prompt.title,
            message: prompt.message,
            placeholder: prompt.placeholder,
            open_title: prompt.open_title,
            cancel_title: prompt.cancel_title,
        }
    }
}
