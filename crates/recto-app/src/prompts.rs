//! **The sheets that ask the person something:** a password for a protected file ([`crate::passwords`]) and whether to open an address a link goes to ([`crate::linking`]). Each
//! asking has an id of its own, so an answer goes to the sheet that was shown and never to one that came after it.

use crate::linking::{LinkPrompt, prompt_for};
use crate::lock;
use crate::passwords::Asks;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Default)]
pub struct Prompts {
    /// The files waiting for a password, in turn. The app's own: the bridge answers a sheet by [`Self::password_answered`] alone.
    pub(crate) passwords: Mutex<Asks>,
    /// The address a link asked to open, while its sheet is up. The bridge answers it by [`Self::link_answered`] alone.
    pub(crate) link: Mutex<Option<LinkPrompt>>,
    /// How many link sheets were asked for, for their ids.
    links_made: AtomicU64,
}

impl Prompts {
    /// A link asked to open `address`: its sheet, with the next id, in place of any before it.
    pub(crate) fn ask_link(&self, address: &str) {
        let id = self.links_made.fetch_add(1, Ordering::SeqCst) + 1;
        *lock(&self.link) = Some(prompt_for(id, address));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: two link sheets share an id, or a second link's sheet doesn't replace the first, so an answer to the sheet on screen opens the address of one that was asked before it.
    #[test]
    fn each_link_sheet_has_an_id_of_its_own_and_the_last_one_is_shown() {
        let prompts = Prompts::default();
        prompts.ask_link("https://a.example/");
        let first = lock(&prompts.link).clone().expect("a sheet");
        prompts.ask_link("mailto:b@example.com");
        let second = lock(&prompts.link).clone().expect("a sheet");
        assert_ne!(first.id, second.id);
        assert_eq!(second.address, "mailto:b@example.com");
        assert!(second.id > first.id, "ids only grow");
    }

    /// Failure: an answer to a link's sheet opens an address the sheet on screen didn't show (an earlier sheet's answer arriving after a later link asked),
    /// Cancel opens it anyway, or the sheet stays up after its answer: what the bridge's `link_prompt_answered` did, now the prompts' own (R3.7, S2).
    #[test]
    fn a_link_sheets_answer_is_for_the_sheet_on_screen() {
        let core = crate::queue::CoreQueue::default();
        let prompts = Prompts::default();
        prompts.ask_link("https://a.example/");
        let first = lock(&prompts.link).clone().expect("a sheet").id;
        prompts.ask_link("https://b.example/");
        let second = lock(&prompts.link).clone().expect("a sheet").id;
        assert_eq!(
            prompts.link_answered(first, true, &core),
            None,
            "the first sheet's answer, after the second asked"
        );
        assert!(lock(&prompts.link).is_some(), "the second sheet stays for its own answer");
        assert_eq!(prompts.link_answered(second, true, &core).as_deref(), Some("https://b.example/"));
        assert!(lock(&prompts.link).is_none(), "answered, it goes");
        prompts.ask_link("https://c.example/");
        let third = lock(&prompts.link).clone().expect("a sheet").id;
        assert_eq!(prompts.link_answered(third, false, &core), None, "Cancel opens nothing");
        assert!(lock(&prompts.link).is_none(), "and the sheet goes");
    }
}
