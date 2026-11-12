//! **The sheets that ask the person something:** a password for a protected file ([`crate::passwords`]) and whether to open an address a link goes to ([`crate::linking`]). Each
//! asking has an id of its own, so an answer goes to the sheet that was shown and never to one that came after it.

use crate::linking::{LinkPrompt, prompt_for};
use crate::lock;
use crate::passwords::Asks;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Default)]
pub struct Prompts {
    /// The files waiting for a password, in turn.
    pub passwords: Mutex<Asks>,
    /// The address a link asked to open, while its sheet is up.
    pub link: Mutex<Option<LinkPrompt>>,
    /// How many link sheets were asked for, for their ids.
    links_made: AtomicU64,
}

impl Prompts {
    /// A link asked to open `address`: its sheet, with the next id, in place of any before it.
    pub fn ask_link(&self, address: &str) {
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
}
