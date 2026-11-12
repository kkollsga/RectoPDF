//! **The password prompt: who's asked, what it says, and how many tries.** Rust decides; the host draws the system's sheet with a secure field and hands back what was typed, once, with the id of the prompt it showed.
//!
//! A password is used to open the file and draw it, held in memory while its tab is open (`recto_reader::engine::Keyring`), and never stored, logged, journaled or put in the state record. A restored protected tab asks again. An agent can't supply or read one: no command takes or gives it, and nothing read from a file opened with one goes to an agent (`recto_session::Session::withheld_from_agents`).
//!
//! Several files can want a password at once, as two protected files opened together do. Each is an [`Ask`] with an id of its own, waiting in turn in [`Asks`]; the sheet shows the first, and an answer goes to the ask whose id came back with it, never to whatever's first by then.

use recto_session::TabId;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// Wrong passwords in a row before it stops asking, for that file, until it's opened again.
pub const MAX_TRIES: u32 = 5;

/// Files waiting to be asked about at once. More protected files than this opened together wait to be opened again.
pub const MAX_ASKS: usize = 8;

/// Files whose wrong tries are remembered, so going to another tab doesn't start them again: one per protected file a person's working with is plenty.
const MAX_REMEMBERED: usize = 32;

/// What the asking is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A file being opened, which has no tab yet.
    File,
    /// A tab that came back from the record, or was left, needing its password again.
    Tab(TabId),
}

/// A password being asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    /// Which ask this is, for the answer to come back to: the sheet hands it back with what was typed.
    pub id: u64,
    pub target: Target,
    pub path: PathBuf,
    /// Wrong passwords so far.
    pub wrong: u32,
    /// It's being tried now, so the sheet isn't shown again meanwhile.
    pub trying: bool,
}

/// **Who's waiting to be asked for a password,** in turn, and what each file's tries have come to. On the session thread's side, behind one short lock.
#[derive(Debug, Default)]
pub struct Asks {
    waiting: VecDeque<Ask>,
    next_id: u64,
    /// Wrong tries so far for files not yet opened with their password, which last until the file's opened again (SPEC), whichever tab is visited meanwhile.
    tries: Vec<(PathBuf, u32)>,
    /// Files whose tries ran out: not asked about again until they're opened again.
    stopped: Vec<PathBuf>,
    /// A protected tab the person said Cancel for: it isn't asked about again until they go to it again.
    declined: Option<TabId>,
}

/// What the answer to an ask came to, for what's said.
#[derive(Debug, PartialEq, Eq)]
pub enum Wrong {
    /// It asks again, with a try fewer.
    AskAgain,
    /// That was the last try, and it's stopped asking.
    Stopped,
    /// The ask had gone already.
    Gone,
}

impl Asks {
    /// Asks for a file's password, after any already waiting, unless it's being asked for already, its tries have run out, or too many are waiting. Whether it's waiting now.
    pub fn ask(&mut self, target: Target, path: &Path) -> bool {
        if self.waiting.iter().any(|ask| ask.path == path) {
            return true;
        }
        if self.stopped.iter().any(|known| known == path) || self.waiting.len() >= MAX_ASKS {
            return false;
        }
        self.next_id += 1;
        let wrong = self.tries.iter().find(|(known, _)| known == path).map_or(0, |(_, wrong)| *wrong);
        self.waiting.push_back(Ask {
            id: self.next_id,
            target,
            path: path.to_path_buf(),
            wrong,
            trying: false,
        });
        true
    }

    /// The ask the sheet shows now: the first, unless it's being tried.
    pub fn showing(&self) -> Option<&Ask> {
        self.waiting.front().filter(|ask| !ask.trying)
    }

    /// Whether a password is being tried now, on its own thread: what a test waits for before it looks.
    pub fn trying(&self) -> bool {
        self.waiting.iter().any(|ask| ask.trying)
    }

    /// The sheet's words for the ask showing.
    pub fn prompt(&self) -> Option<PasswordPrompt> {
        self.showing().map(prompt_for)
    }

    /// Cancel on the sheet for this ask: it goes, and a tab's isn't asked about again till the person goes to it again.
    pub fn cancel(&mut self, id: u64) {
        if let Some(ask) = self.take(id)
            && let Target::Tab(tab) = ask.target
        {
            self.declined = Some(tab);
        }
    }

    /// What was typed for this ask is being tried: it's what's tried, and nothing's shown meanwhile. `None` if it's gone, or it's being tried already.
    pub fn start_trying(&mut self, id: u64) -> Option<Ask> {
        let ask = self.waiting.iter_mut().find(|ask| ask.id == id && !ask.trying)?;
        ask.trying = true;
        Some(ask.clone())
    }

    /// A wrong password for this ask, or something no password could be: a try fewer, and after the last it stops asking about that file until it's opened again.
    pub fn wrong(&mut self, id: u64) -> Wrong {
        let Some(at) = self.waiting.iter().position(|ask| ask.id == id) else {
            return Wrong::Gone;
        };
        let wrong = self.waiting[at].wrong + 1;
        let path = self.waiting[at].path.clone();
        self.remember_tries(&path, wrong);
        if wrong >= MAX_TRIES {
            if let Some(Ask { target: Target::Tab(tab), .. }) = self.waiting.remove(at) {
                self.declined = Some(tab);
            }
            self.tries.retain(|(known, _)| *known != path);
            if self.stopped.len() >= MAX_REMEMBERED {
                self.stopped.remove(0);
            }
            self.stopped.push(path);
            return Wrong::Stopped;
        }
        let ask = &mut self.waiting[at];
        ask.wrong = wrong;
        ask.trying = false;
        Wrong::AskAgain
    }

    /// The password opened the file: its ask goes, and so do its tries.
    pub fn opened(&mut self, id: u64) -> Option<Ask> {
        let ask = self.take(id)?;
        self.tries.retain(|(known, _)| *known != ask.path);
        Some(ask)
    }

    /// Trying failed for a reason that isn't the password, which is said another way: the ask goes.
    pub fn failed(&mut self, id: u64) {
        self.take(id);
    }

    /// **The file's opened again,** from File > Open, Open Recent or a drop: what its tries came to is forgotten, and a Cancel for its tab too, so it's asked afresh.
    pub fn opened_again(&mut self, paths: &[PathBuf], tab: Option<TabId>) {
        self.tries.retain(|(known, _)| !paths.contains(known));
        self.stopped.retain(|known| !paths.contains(known));
        if tab.is_some() && self.declined == tab {
            self.declined = None;
        }
    }

    /// The tabs being asked about, for the session to say which have closed.
    pub fn tabs_asked(&self) -> Vec<TabId> {
        self.waiting
            .iter()
            .filter_map(|ask| if let Target::Tab(tab) = ask.target { Some(tab) } else { None })
            .collect()
    }

    /// What's showing now, from the session: a Cancel lasts until the person goes to another tab, and asks for tabs that have closed go.
    pub fn showing_tab(&mut self, shown: Option<TabId>, closed: &[TabId]) {
        if self.declined.is_some() && self.declined != shown {
            self.declined = None;
        }
        self.waiting.retain(|ask| !matches!(ask.target, Target::Tab(tab) if closed.contains(&tab)));
    }

    /// Whether the person said Cancel for this tab and hasn't gone to it again.
    pub fn declined(&self, tab: TabId) -> bool {
        self.declined == Some(tab)
    }

    fn take(&mut self, id: u64) -> Option<Ask> {
        let at = self.waiting.iter().position(|ask| ask.id == id)?;
        self.waiting.remove(at)
    }

    fn remember_tries(&mut self, path: &Path, wrong: u32) {
        if let Some(known) = self.tries.iter_mut().find(|(known, _)| known == path) {
            known.1 = wrong;
            return;
        }
        if self.tries.len() >= MAX_REMEMBERED {
            self.tries.remove(0);
        }
        self.tries.push((path.to_path_buf(), wrong));
    }
}

/// The sheet's words, and which ask they're for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PasswordPrompt {
    /// Handed back with the answer, so it goes to this file and no other.
    pub id: u64,
    pub title: String,
    pub message: String,
    pub placeholder: String,
    pub open_title: String,
    pub cancel_title: String,
}

/// What the sheet says, for this file and how many tries there have been.
pub fn prompt_for(ask: &Ask) -> PasswordPrompt {
    let name = ask
        .path
        .file_name()
        .map(|name| recto_core::clean_line(&name.to_string_lossy()))
        .unwrap_or_default();
    let again = if matches!(ask.target, Target::Tab(_)) {
        " Passwords aren't kept, so it's asked again after RectoPDF quits."
    } else {
        " It's used to open the file and isn't kept."
    };
    let message = if ask.wrong == 0 {
        format!("Type the password for \u{201C}{name}\u{201D}.{again}")
    } else {
        let left = MAX_TRIES.saturating_sub(ask.wrong);
        format!(
            "That password didn't open \u{201C}{name}\u{201D}. {left} {} left.",
            if left == 1 { "try" } else { "tries" }
        )
    };
    PasswordPrompt {
        id: ask.id,
        title: "This PDF needs a password".into(),
        message,
        placeholder: "Password".into(),
        open_title: "Open".into(),
        cancel_title: "Cancel".into(),
    }
}

/// What's said when a file opened with its password can't be told apart from another at its path, so its password can't be kept for drawing it.
pub const CANT_KEEP: &str = "This file opened with its password, but RectoPDF can't tell it apart from another file at its place on this disk, so it can't keep the password to draw it.";

/// What's said when the tries have run out.
pub fn stopped_asking() -> String {
    format!("That's {MAX_TRIES} wrong passwords, so RectoPDF stopped asking. Open the file again to try again.")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: the sheet doesn't say whose password it wants, doesn't say it isn't kept, or doesn't count down the tries that are left.
    #[test]
    fn the_prompt_names_the_file_and_counts_the_tries() {
        let mut ask = Ask {
            id: 1,
            target: Target::File,
            path: PathBuf::from("/d/secret\u{202E}.pdf"),
            wrong: 0,
            trying: false,
        };
        let first = prompt_for(&ask);
        assert!(
            first.message.contains("\u{201C}secret.pdf\u{201D}") && first.message.contains("isn't kept"),
            "{}",
            first.message
        );
        ask.wrong = 3;
        assert!(prompt_for(&ask).message.contains("2 tries left"));
        ask.wrong = 4;
        assert!(prompt_for(&ask).message.contains("1 try left"));
        let tab = prompt_for(&Ask {
            id: 2,
            target: Target::Tab(TabId(1)),
            path: PathBuf::from("/d/a.pdf"),
            wrong: 0,
            trying: false,
        });
        assert!(tab.message.contains("asked again after RectoPDF quits"));
        assert!(stopped_asking().contains(&MAX_TRIES.to_string()));
    }

    /// Failure: with two protected files asking at once, a password typed for the one showing goes to the other, the second ask overwrites the first, or an answer to a prompt that's gone does something (review O2).
    #[test]
    fn two_files_asking_at_once_each_get_their_own_answer() {
        let mut asks = Asks::default();
        assert!(asks.ask(Target::File, Path::new("/d/a.pdf")));
        assert!(asks.ask(Target::File, Path::new("/d/b.pdf")));
        assert!(asks.ask(Target::File, Path::new("/d/a.pdf")), "asked once");
        let first = asks.prompt().unwrap();
        assert!(first.message.contains("a.pdf"), "the first opened is asked about first: {}", first.message);
        let trying = asks.start_trying(first.id).unwrap();
        assert_eq!(trying.path, Path::new("/d/a.pdf"));
        assert!(asks.prompt().is_none(), "nothing's shown while it's tried");
        assert!(asks.start_trying(first.id).is_none(), "an answer can't be tried twice");
        assert_eq!(asks.opened(first.id).unwrap().path, Path::new("/d/a.pdf"));
        let second = asks.prompt().unwrap();
        assert!(second.message.contains("b.pdf") && second.id != first.id, "{second:?}");
        assert!(asks.start_trying(first.id).is_none(), "an answer to a prompt that's gone goes nowhere");
        asks.cancel(second.id);
        assert!(asks.prompt().is_none());
        for at in 0..MAX_ASKS + 3 {
            asks.ask(Target::File, &PathBuf::from(format!("/d/many{at}.pdf")));
        }
        assert_eq!(asks.waiting.len(), MAX_ASKS, "capped");
    }

    /// Failure: the five tries start again when the person goes to another tab and back, or after a Cancel, rather than lasting until the file's opened again (SPEC; review O8); or a file that's stopped asking asks again by itself.
    #[test]
    fn the_tries_last_until_the_file_is_opened_again() {
        let mut asks = Asks::default();
        let path = Path::new("/d/locked.pdf");
        let tab = TabId(4);
        asks.ask(Target::Tab(tab), path);
        for _ in 0..3 {
            let id = asks.prompt().unwrap().id;
            asks.start_trying(id);
            assert_eq!(asks.wrong(id), Wrong::AskAgain);
        }
        let id = asks.prompt().unwrap().id;
        asks.cancel(id);
        assert!(asks.declined(tab));
        asks.showing_tab(Some(TabId(9)), &[]);
        assert!(!asks.declined(tab), "going elsewhere ends a Cancel");
        asks.ask(Target::Tab(tab), path);
        assert!(
            asks.prompt().unwrap().message.contains("2 tries left"),
            "the tries are where they were: {:?}",
            asks.prompt()
        );
        for left in [1, 0] {
            let id = asks.prompt().unwrap().id;
            asks.start_trying(id);
            assert_eq!(asks.wrong(id), if left == 0 { Wrong::Stopped } else { Wrong::AskAgain });
        }
        asks.showing_tab(Some(TabId(9)), &[]);
        assert!(!asks.ask(Target::Tab(tab), path) && asks.prompt().is_none(), "stopped, whichever tab's visited");
        asks.opened_again(&[path.to_path_buf()], Some(tab));
        assert!(asks.ask(Target::Tab(tab), path));
        assert!(!asks.prompt().unwrap().message.contains("left"), "opened again, it's asked afresh");
    }
}
