//! **The password prompt: who's asked, what it says, and how many tries.** Rust decides; the host draws the system's sheet with a secure field and hands back what was typed, once.
//!
//! A password is used to open the file and draw it, held in memory while its tab is open (`recto_reader::engine::Keyring`), and never stored, logged, journaled or put in the state record. A restored protected tab asks again. An agent can't supply or read one: no command takes or gives it, and an agent's own reads of the file, which open it with no password, are refused as before.

use recto_session::TabId;
use std::path::{Path, PathBuf};

/// Wrong passwords in a row before it stops asking, for that file, until it's opened again.
pub const MAX_TRIES: u32 = 5;

/// What the asking is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// A file being opened, which has no tab yet.
    File,
    /// A tab that came back from the record, or was left, needing its password again.
    Tab(TabId),
}

/// A password being asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ask {
    pub target: Target,
    pub path: PathBuf,
    /// Wrong passwords so far.
    pub wrong: u32,
    /// It's being tried now, so the sheet isn't shown again meanwhile.
    pub trying: bool,
}

impl Ask {
    pub fn new(target: Target, path: &Path) -> Self {
        Self { target, path: path.to_path_buf(), wrong: 0, trying: false }
    }
}

/// The sheet's words.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PasswordPrompt {
    pub title: String,
    pub message: String,
    pub placeholder: String,
    pub open_title: String,
    pub cancel_title: String,
}

/// What the sheet says, for this file and how many tries there have been.
pub(crate) fn prompt_for(ask: &Ask) -> PasswordPrompt {
    let name = ask.path.file_name().map(|name| recto_core::clean_line(&name.to_string_lossy())).unwrap_or_default();
    let again = if matches!(ask.target, Target::Tab(_)) { " Passwords aren't kept, so it's asked again after RectoPDF quits." } else { " It's used to open the file and isn't kept." };
    let message = if ask.wrong == 0 {
        format!("Type the password for \u{201C}{name}\u{201D}.{again}")
    } else {
        let left = MAX_TRIES.saturating_sub(ask.wrong);
        format!("That password didn't open \u{201C}{name}\u{201D}. {left} {} left.", if left == 1 { "try" } else { "tries" })
    };
    PasswordPrompt { title: "This PDF needs a password".into(), message, placeholder: "Password".into(), open_title: "Open".into(), cancel_title: "Cancel".into() }
}

/// What's said when the tries have run out.
pub(crate) fn stopped_asking() -> String {
    format!("That's {MAX_TRIES} wrong passwords, so RectoPDF stopped asking. Open the file again to try again.")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: the sheet doesn't say whose password it wants, doesn't say it isn't kept, or doesn't count down the tries that are left.
    #[test]
    fn the_prompt_names_the_file_and_counts_the_tries() {
        let mut ask = Ask::new(Target::File, Path::new("/d/secret\u{202E}.pdf"));
        let first = prompt_for(&ask);
        assert!(first.message.contains("\u{201C}secret.pdf\u{201D}") && first.message.contains("isn't kept"), "{}", first.message);
        ask.wrong = 3;
        assert!(prompt_for(&ask).message.contains("2 tries left"));
        ask.wrong = 4;
        assert!(prompt_for(&ask).message.contains("1 try left"));
        let tab = prompt_for(&Ask::new(Target::Tab(TabId(1)), Path::new("/d/a.pdf")));
        assert!(tab.message.contains("asked again after RectoPDF quits"));
        assert!(stopped_asking().contains(&MAX_TRIES.to_string()));
    }
}
