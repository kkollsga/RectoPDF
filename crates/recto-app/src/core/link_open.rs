//! **A link opened from outside** (PLAN 8.4): a `rectopdf:` link clicked in Notes, Mail, Finder or a web page, and a file the Finder hands over with a
//! `#page=N` part. It is the person's open, as a double-click is: the system acts for them, so it goes through [`super::opening::open_for_person`] and is
//! the person's `open {path, page}`, run by the one executor like any open. The link is read here by `recto_core::locator`, so the executor never holds
//! it; it is dropped as soon as it is read, and never logged, kept or shown.
//!
//! Hostile by construction (any page can send one): a link opens one local PDF, read-only, at a page, and nothing else; at most [`Arrivals::MAX_OUTSIDE`]
//! of them are taken in [`Arrivals::SPAN_MS`]; a bad one is refused in Rust's constant words, which never repeat any part of it.

use super::Inner;
use super::answer_parts::AnswerParts;
use super::opening::open_for_person;
use super::person;
use recto_agent::commands::Move;
use recto_agent::commands::tabs::Open;
use recto_agent::{Code, Command};
use recto_core::locator::{read_address, read_page_fragment};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// What's said when more links come than are opened.
const TOO_MANY: &str = "More links arrived at once than RectoPDF opens, so the rest weren't opened.";

/// **When links from outside arrived,** to keep their number bounded: no program, page or loop can open files without end by sending links.
#[derive(Default)]
pub(in crate::core) struct Arrivals {
    at: Mutex<VecDeque<u64>>,
    told: AtomicBool,
}

impl Arrivals {
    /// The most links from outside taken in [`Self::SPAN_MS`].
    const MAX_OUTSIDE: usize = 8;
    /// The span, in milliseconds, the cap is counted over.
    const SPAN_MS: u64 = 10_000;

    /// Whether a link that arrived at `now_ms` is taken: true while fewer than the cap have arrived in the span, and then it's counted.
    fn arrived(&self, now_ms: u64) -> bool {
        let mut at = super::lock(&self.at);
        while at.front().is_some_and(|first| now_ms.saturating_sub(*first) >= Self::SPAN_MS) {
            at.pop_front();
        }
        if at.is_empty() {
            self.told.store(false, Ordering::SeqCst);
        }
        if at.len() >= Self::MAX_OUTSIDE {
            return false;
        }
        at.push_back(now_ms);
        true
    }
}

/// `UiAction::OpenLink`: the link the system handed over, as sent. Read here, then dropped.
pub fn from_outside(inner: &Inner, link: String) {
    let notices = inner.window.notices();
    if !inner.window.links.arrived(inner.life.now_ms()) {
        if !inner.window.links.told.swap(true, Ordering::SeqCst) {
            notices.fail_open(TOO_MANY.into());
        }
        return;
    }
    match read_address(&link) {
        Ok(address) => open_for_person(&inner.life, &inner.core, address.path, address.file_page),
        Err(why) => notices.fail_open(format!("This link couldn't be opened. {}", why.message())),
    }
}

/// `UiAction::OpenAt`: a file the system handed over with what came after `#`. No cap: it's the Finder's open, not a link from a page.
pub fn open_at(inner: &Inner, path: PathBuf, fragment: &str) {
    match read_page_fragment(fragment) {
        Ok(page) => open_for_person(&inner.life, &inner.core, path, page),
        Err(why) => inner.window.notices().fail_open(why.message().into()),
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
