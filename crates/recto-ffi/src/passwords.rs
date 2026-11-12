//! **The password prompt's record,** as the host draws it. Who's asked, what it says and how many tries are `recto_app::passwords`'s.

pub use recto_app::passwords::PasswordPrompt;
pub(crate) use recto_app::passwords::{Wrong, stopped_asking};

/// The sheet's words, and which ask they're for.
#[uniffi::remote(Record)]
pub struct PasswordPrompt {
    /// Handed back with the answer, so it goes to this file and no other.
    pub id: u64,
    pub title: String,
    pub message: String,
    pub placeholder: String,
    pub open_title: String,
    pub cancel_title: String,
}
