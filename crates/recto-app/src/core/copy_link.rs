//! **The window's Copy Link** (PLAN 8.4, owner 2026-11-26): Edit > Copy Link, the page's right-click menu and a contents row's right-click, which put the
//! file's whole address with the page (`file:///…/manual.pdf#page=N`) on the clipboard as plain text, for the person alone. Each sends the person's
//! `recto link <page> --to-clipboard`, the command an agent's `recto link` is, run by the one executor; the address is made in `recto_core::locator`, and
//! handed to the window's clipboard by `recto_agent`'s sink. An agent is never given it, at any level, and the answer holds no address and no path.
//!
//! **What Copy Link doesn't promise:** Preview may open the file at its first page. Nothing here opens a file; the page is in the address for a reader that
//! takes it. So it's said when the link is copied ([`answered`]), in one sentence after the confirmation (owner, 2026-11-26: "Build it, say so").

use crate::core::answer_parts::AnswerParts;
use crate::core::menu_rows::{Agents, Asked, Row};
use crate::core::person;
use crate::core::row_parts::RowParts;
use crate::menus::HostAction;
use recto_agent::commands::Read;
use recto_agent::commands::page_link::Link;
use recto_agent::{Code, Command, TabRef};
use recto_session::TabId;
use std::collections::VecDeque;

/// Edit > Copy Link's id, and the page's right-click menu's.
pub const COPY_LINK_ID: &str = "edit.copy_link";

/// What the menu item says.
pub const TITLE: &str = "Copy Link";
/// Said after the confirmation: the page is in the address, and Preview may not take it.
const PREVIEW: &str = "Preview may open it at the first page.";

/// **The Copy Links asked and not yet landed, in the order they were sent** (the person's `Link` runs on the session thread in that order): each one's place
/// among what's asked of the clipboard ([`super::snapshot_tool::Tool::link_asked`]), taken off the front as its address lands or it's refused; and whether
/// each address that landed was put, for [`answered`] to say.
#[derive(Default)]
pub struct Links {
    asked: VecDeque<u64>,
    put: VecDeque<bool>,
}

impl Links {
    /// A Copy Link asked, at `ticket`.
    pub fn ask(&mut self, ticket: u64) {
        self.asked.push_back(ticket);
    }

    /// The newest ask handed back: its command wasn't taken, so nothing will answer it.
    pub fn take_back(&mut self) {
        self.asked.pop_back();
    }

    /// The oldest ask's address has landed: its ticket, or none for a route that didn't ask here.
    pub fn land(&mut self) -> Option<u64> {
        self.asked.pop_front()
    }

    /// Whether the address that just landed was put on the clipboard.
    pub fn landed(&mut self, put: bool) {
        self.put.push_back(put);
    }

    /// The oldest ask has been answered: for a refusal its ask is dropped, and for a Copy Link that went through whether its address was put (true for one
    /// that landed by no route that says).
    pub fn answered(&mut self, copied: bool) -> bool {
        if copied {
            let put = self.put.pop_front();
            if put.is_none() {
                self.asked.pop_front();
            }
            return put.unwrap_or(true);
        }
        self.asked.pop_front();
        true
    }
}

/// The person's `recto link` of `page` (from 1) of `tab`, to the window's clipboard.
pub fn link_of(tab: u32, page: u32) -> Command {
    Command::Read(Read::Link(Link {
        page,
        tab: TabRef::Id(TabId(tab)),
        to_clipboard: true,
    }))
}

/// **The menu id** ([`super::menu_rows`]): the person's selection's first page, which only they have. An agent's palette run is refused, naming the command
/// it has for itself, which never gives it the address.
pub fn row<'a>(parts: &RowParts<'a>, asked: &Asked<'_>) -> Option<Row<'a>> {
    if asked.id != COPY_LINK_ID {
        return None;
    }
    let parts = *parts;
    Some(Row::window(
        "the address goes to the person's own clipboard, which an agent is never given; the selection is theirs alone",
        Agents::ItsOwn("`recto link`, which names the file and not its folder"),
        move || {
            if let Some((tab, Some(range))) = parts.selected() {
                parts.link_asked();
                if !parts.person().submit(link_of(tab, range.start.page + 1)) {
                    parts.link_not_taken();
                }
            }
            HostAction::None
        },
    ))
}

/// **A contents row's Copy Link** ([`super::controls::UiAction::CopyLink`]): the page a tap on that row goes to ([`super::navigation::contents_page`]).
pub fn of_row(person: person::Person<'_>, tools: &super::Tools, panes: &crate::panes::Panes, engine: &recto_reader::Engine, row: &crate::SidebarRow) {
    let Some(page) = row.page else { return };
    let page = super::navigation::contents_page(page, super::navigation::shown_pages(panes, engine, TabId(row.tab)));
    tools.tool().link_asked();
    if !person.submit(link_of(row.tab, page)) {
        tools.tool().link_not_taken();
    }
}

/// **What a right-click on a contents row offers** (owner 2026-11-26): Copy Link, for a row that has a page, and nothing for one that has none. The host draws it.
pub fn contents_menu(row: &crate::SidebarRow) -> Vec<crate::menus::MenuEntry> {
    if row.page.is_none() {
        return Vec::new();
    }
    crate::menus::context_menu(None, None)
        .into_iter()
        .filter(|entry| entry.id == COPY_LINK_ID)
        .collect()
}

/// **How a person's Copy Link went, in words for them:** a refusal said by the page, as an agent's words are never shown to the person, and a quiet line when
/// the page copied isn't the tab's own number.
pub fn answered(parts: &AnswerParts<'_>, ran: &person::Ran<'_>) {
    let Command::Read(Read::Link(Link { to_clipboard: true, page, .. })) = ran.command else {
        return;
    };
    let mut put = true;
    parts.snapshot_answered(|tool| {
        put = tool.link_answered(ran.result.is_ok());
        false
    });
    if let Ok(answer) = ran.result {
        // Nothing's said of a copy that a newer ask has taken the clipboard from: the person is told of what's on it.
        if !put {
            return;
        }
        let file_page = answer["file_page"].as_u64().unwrap_or(0);
        let of = if file_page == u64::from(*page) { "" } else { " of the saved file" };
        parts.say(format!("Copied a link to page {file_page}{of}. {PREVIEW}"));
        return;
    }
    let Some(error) = ran.refused() else { return };
    let words = if error.code == Code::NO_ADDRESS {
        "This file's location can't be made into a link."
    } else if error.code == Code::NOT_REFERENCEABLE {
        "That page came from another PDF and has no place in this file until you save."
    } else if error.code == Code::SOURCE_CHANGED {
        "That page can't be read now: its file has changed or gone."
    } else if error.code == Code::NO_SUCH_PAGE {
        "There's no such page."
    } else {
        match person::session_words(error) {
            Some(words) => words,
            None => return,
        }
    };
    parts.say(format!("Couldn't copy the link: {words}"));
}
