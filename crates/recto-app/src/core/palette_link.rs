//! **A link pasted into ⌘K's field** (owner, 2026-11-27): when what's typed is a link [`read_address`] accepts, the palette's first row is *Open link: name,
//! page N*, and choosing it is the person's `open {path, page}`, the very open a link from outside is ([`super::link_open`], through
//! [`super::opening::open_for_person`]), with the same answered words. The cap on links from outside doesn't apply: this is the person's own paste, not a
//! page sending links. A link that's refused ([`AddressError::Path`] or `Page`) shows one row that can't be chosen, with Rust's constant words, never any part
//! of the link. Plain words (not a link at all) add nothing. An agent's `palette --run` of the row is its own `open`, at the level `open` needs.
//!
//! The row's id is `link:` and the link as typed; the link is read again, by the same `read_address`, when the row is run.

use super::{CoreQueue, HostAction, Lifecycle};
use crate::palette::{PaletteRow, link_row};
use recto_agent::Command;
use recto_agent::commands::tabs::Open;
use recto_core::locator::{AddressError, read_address};

const PREFIX: &str = "link:";
/// What the id of the row that can't be chosen starts with, then the refusal's code: nothing of the link.
const REFUSED: &str = "link:refused:";
/// The most of a file's name the row shows.
const NAME_CHARS: usize = 60;

/// The file's name as the row shows it: cleaned as any name from outside is, cut with "…".
fn name_of(path: &std::path::Path) -> String {
    let name = recto_core::clean_line(&path.file_name().unwrap_or_default().to_string_lossy());
    if name.chars().count() > NAME_CHARS {
        format!("{}…", name.chars().take(NAME_CHARS - 1).collect::<String>())
    } else {
        name
    }
}

/// How many characters of what's typed are kept: a link's start says it's one, and then all that `read_address` reads, so a pasted link isn't cut to a
/// shorter name (a link longer than that is refused by `read_address`, in its words); anything else keeps the palette's own limit.
pub fn limit_for(text: &str) -> usize {
    let head: String = text.trim_start().chars().take(10).collect::<String>().to_ascii_lowercase();
    if head.starts_with("rectopdf:") || head.starts_with("file:") {
        recto_core::locator::MAX_READ_BYTES + 1
    } else {
        recto_session::palette::MAX_QUERY_CHARS * 4
    }
}

/// Puts the link's row first when `query` is a link, or the row that can't be chosen when it's one that can't be read.
pub fn offer(query: &str, rows: &mut Vec<PaletteRow>) {
    let row = match read_address(query) {
        Ok(address) => {
            let name = name_of(&address.path);
            let title = match address.file_page {
                Some(page) => format!("Open link: {name}, page {page}"),
                None => format!("Open link: {name}"),
            };
            link_row(format!("{PREFIX}{}", query.trim()), title, true)
        }
        Err(AddressError::NotALink) => return,
        Err(why) => link_row(format!("{REFUSED}{}", why.code()), why.message().into(), false),
    };
    rows.insert(0, row);
}

/// The `open` a link row's id comes to, for an agent's run: its own command, checked as `open` is.
pub fn as_command(id: &str) -> Option<Command> {
    let address = read_address(id.strip_prefix(PREFIX)?).ok()?;
    Some(
        Open {
            path: address.path,
            page: address.file_page,
        }
        .into(),
    )
}

/// An agent's run of the row that can't be chosen: the link's own refusal, its code and Rust's words, as `open --link` gives it.
pub fn refusal(id: &str) -> Option<recto_agent::AgentError> {
    let code = id.strip_prefix(REFUSED)?;
    let why = [AddressError::NotALink, AddressError::Path, AddressError::Page]
        .into_iter()
        .find(|why| why.code() == code)?;
    Some(recto_agent::AgentError::new(
        recto_agent::Code::lower(why.code()),
        why.message(),
        recto_agent::exit::USAGE,
    ))
}

/// The person's run of a row: `None` when it isn't a link's. The refused row does nothing, and leaves the palette as it is.
pub fn run(palette: &crate::palette::Palette, life: &Lifecycle, core: &CoreQueue, id: &str) -> Option<HostAction> {
    if id.starts_with(REFUSED) {
        return Some(HostAction::None);
    }
    let address = read_address(id.strip_prefix(PREFIX)?).ok()?;
    palette.close();
    super::opening::open_for_person(life, core, address.path, address.file_page);
    Some(HostAction::None)
}
