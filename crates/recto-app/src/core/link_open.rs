//! **A link opened from outside** (PLAN 8.4): a `rectopdf:` link clicked in Notes, Mail, Finder or a web page, and a file the Finder hands over with a
//! `#page=N` part. It is the person's open, as a double-click is: the system acts for them, so it goes through [`super::opening::open_for_person`] and is
//! the person's `open {path, page}`, run by the one executor like any open. The link is read here by `recto_core::locator`, so the executor never holds
//! it; it is dropped as soon as it is read, and never logged, kept or shown.
//!
//! Hostile by construction (any page can send one): a link opens one local PDF, read-only, at a page, and nothing else; at most [`Arrivals::MAX_OUTSIDE`]
//! of them are taken in [`Arrivals::SPAN_MS`]; a bad one is refused in Rust's constant words, which never repeat any part of it.

use super::answer_parts::AnswerParts;
use super::opening::open_for_person;
use super::person;
use super::window::Window;
use super::{CoreQueue, Lifecycle};
use recto_agent::commands::Move;
use recto_agent::commands::tabs::Open;
use recto_agent::{Code, Command};
use recto_core::locator::{read_address, read_page_fragment};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;

/// What's said when more links come than are opened.
const TOO_MANY: &str = "More links arrived at once than RectoPDF opens, so the rest weren't opened.";

/// What became of one arrival.
#[derive(Debug, PartialEq, Eq)]
enum Arrival {
    /// Within the cap: counted, and read.
    Taken,
    /// Past the cap. `first` is true for exactly one arrival of a flood: the one that says so to the person.
    Refused { first: bool },
}

/// The arrival times in the span, and whether the person has been told of this flood. One lock holds both, so whether an arrival is the first refused of a
/// flood is decided where the queue is changed, and two threads at the edge of the cap can't both be (or neither).
#[derive(Default)]
struct Recent {
    at: VecDeque<u64>,
    told: bool,
}

/// **When links from outside arrived,** to keep their number bounded: no program, page or loop can open files without end by sending links.
#[derive(Default)]
pub(in crate::core) struct Arrivals {
    state: Mutex<Recent>,
}

impl Arrivals {
    /// The most links from outside taken in [`Self::SPAN_MS`].
    const MAX_OUTSIDE: usize = 8;
    /// The span, in milliseconds, the cap is counted over.
    const SPAN_MS: u64 = 10_000;

    /// What becomes of a link that arrived at `now_ms`: taken while fewer than the cap have arrived in the span (and then counted), else refused, the first
    /// refusal of a flood being said so. A flood ends when the queue has emptied.
    fn arrived(&self, now_ms: u64) -> Arrival {
        let mut state = super::lock(&self.state);
        while state.at.front().is_some_and(|first| now_ms.saturating_sub(*first) >= Self::SPAN_MS) {
            state.at.pop_front();
        }
        if state.at.is_empty() {
            state.told = false;
        }
        if state.at.len() >= Self::MAX_OUTSIDE {
            let first = !state.told;
            state.told = true;
            return Arrival::Refused { first };
        }
        state.at.push_back(now_ms);
        Arrival::Taken
    }
}

/// `UiAction::OpenLink`: the link the system handed over, as sent. Read here, then dropped.
pub fn from_outside(window: &Window, life: &Lifecycle, core: &CoreQueue, link: String) {
    let notices = window.notices();
    match window.links.arrived(life.now_ms()) {
        Arrival::Taken => {}
        Arrival::Refused { first } => {
            if first {
                notices.fail_open(TOO_MANY.into());
            }
            return;
        }
    }
    match read_address(&link) {
        Ok(address) => open_for_person(life, core, address.path, address.file_page),
        Err(why) => notices.fail_open(format!("This link couldn't be opened. {}", why.message())),
    }
}

/// `UiAction::OpenAt`: a file the system handed over with what came after `#`. No cap: it's the Finder's open, not a link from a page.
pub fn open_at(window: &Window, life: &Lifecycle, core: &CoreQueue, path: PathBuf, fragment: &str) {
    match read_page_fragment(fragment) {
        Ok(page) => open_for_person(life, core, path, page),
        Err(why) => window.notices().fail_open(why.message().into()),
    }
}

/// **What a person's open at a page says of it,** in their words: the page isn't in the tab as it stands, the page's number here differs from the file's, or the
/// file doesn't have the page. Everything else about an open is [`super::opening::answered`]'s. An agent's words are never shown.
pub fn answered(parts: &AnswerParts<'_>, ran: &person::Ran<'_>) {
    let Command::Move(Move::Open(Open { page: Some(_), .. })) = ran.command else {
        return;
    };
    if let Ok(answer) = ran.result {
        let file_page = answer["file_page"].as_u64().unwrap_or(0);
        if answer["landed"].as_bool() == Some(false) {
            parts.say("The link's page isn't in this tab now, after your unsaved edits, so it's shown where it was.".into());
        } else if let Some(page) = answer["page"].as_u64().filter(|page| *page != file_page) {
            parts.say(format!("Page {file_page} of the saved file is page {page} here, after your unsaved edits."));
        }
    } else if ran.refused().is_some_and(|error| error.code == Code::NO_SUCH_PAGE) {
        parts.fail_open("The link is to a page this PDF doesn't have, so nothing was opened.".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: two links at the edge of the cap both say "more arrived than open" (or neither does), so the person is told twice or never. Eight threads
    /// send 50 links each at one instant: exactly 8 are taken and exactly one refusal is the first; after the span the queue has emptied, and the next flood
    /// is told once again.
    #[test]
    fn a_flood_is_taken_eight_times_and_said_exactly_once() {
        let arrivals = Arrivals::default();
        let flood = |at: u64| {
            let (taken, firsts) = std::thread::scope(|scope| {
                let each: Vec<_> = (0..8)
                    .map(|_| {
                        scope.spawn(|| {
                            let (mut taken, mut firsts) = (0, 0);
                            for _ in 0..50 {
                                match arrivals.arrived(at) {
                                    Arrival::Taken => taken += 1,
                                    Arrival::Refused { first: true } => firsts += 1,
                                    Arrival::Refused { first: false } => {}
                                }
                            }
                            (taken, firsts)
                        })
                    })
                    .collect();
                each.into_iter()
                    .map(|each| each.join().expect("a sender"))
                    .fold((0, 0), |(a, b), (c, d)| (a + c, b + d))
            });
            (taken, firsts)
        };
        assert_eq!(flood(1_000), (Arrivals::MAX_OUTSIDE, 1));
        assert_eq!(flood(1_000 + Arrivals::SPAN_MS), (Arrivals::MAX_OUTSIDE, 1), "the next flood is said once more");
    }
}
