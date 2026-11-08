//! **Links, Back and Forward, and pages by their labels,** for an agent: `recto links`, `recto follow`, `recto back`, `recto forward` and `recto goto label`.
//!
//! An agent sees where a link goes and can follow one within the document, as a click does. It never opens a web address: only the person can, in the app, after the app has asked them and shown the real address. A link's address comes from the file, so it goes under `untrusted`, and nothing about a link is ever run.

use crate::{AgentError, Event, SessionAccess, exit, locate, select, where_json};
use recto_core::nav::{LinkTarget, PageLinks};
use recto_session::{DocLoader, Session, TabId};
use serde_json::{Value, json};

/// What a link does, in an agent's terms: where it goes now, with edits counted, or why it doesn't.
pub fn link_json<L: DocLoader>(session: &Session<L>, id: TabId, from: u32, index: usize, rect: [f32; 4], target: &LinkTarget) -> Value {
    let rect = [rect[0], rect[1], rect[2] - rect[0], rect[3] - rect[1]];
    let mut out = json!({ "link": index + 1, "rect": rect });
    match target {
        LinkTarget::Page { page, within } => match session.position_in_same_file(id, from, *page) {
            Some(position) => {
                out["goes_to"] = json!("page");
                out["page"] = json!(position + 1);
                out["within"] = json!(within);
                if let Some(label) = session.page_label_differs(id, position) {
                    out["untrusted"] = json!({ "label": label });
                }
            }
            None => {
                out["goes_to"] = json!("nowhere");
                out["note"] = json!("The page it goes to isn't in the document as it's been edited.");
            }
        },
        LinkTarget::Web(address) => {
            out["goes_to"] = json!("web");
            out["note"] = json!("A web or email address. Only the person using the app can open it, after being asked.");
            out["untrusted"] = json!({ "address": &**address });
        }
        LinkTarget::History { back } => out["goes_to"] = json!(if *back { "back" } else { "forward" }),
        LinkTarget::Refused(why) => {
            out["goes_to"] = json!("refused");
            out["refused"] = json!(why.code());
            out["note"] = json!(why.message());
        }
        LinkTarget::Nowhere => {
            out["goes_to"] = json!("nowhere");
            out["note"] = json!("It names a place that isn't in this document.");
        }
    }
    out
}

/// One page's links, read off the session's lock: a file on a slow disk holds up only this.
fn read<A: SessionAccess>(access: &A, tab: &str, page: u32) -> Result<(TabId, u32, PageLinks), AgentError> {
    let (id, position, source) = locate(access, tab, page)?;
    let read = recto_core::nav::page_links(&source.path, &[source.page]).map_err(AgentError::from)?;
    let links = read.into_iter().next().map(|(_, links)| links).unwrap_or_default();
    Ok((id, position, links))
}

/// `recto links`: what's on a page, and where each goes.
pub fn links<A: SessionAccess>(access: &A, tab: &str, page: u32) -> Result<Value, AgentError> {
    let (id, position, found) = read(access, tab, page)?;
    access.with(|s| {
        let links: Vec<Value> = found.links.iter().enumerate().map(|(index, link)| link_json(s, id, position, index, link.rect, &link.target)).collect();
        Ok(json!({ "tab": id.0, "page": position + 1, "untrusted": { "label": s.page_label_differs(id, position) }, "links": links, "truncated": found.truncated, "note": "`rect` is x, y, width, height in points on the page as the file shows it, from its top left. `recto follow` follows one within the document." }))
    })
}

/// `recto follow`: what a click on a link does, within the document. A web address is the person's to open, and never an agent's.
pub fn follow<A: SessionAccess>(access: &A, tab: &str, page: u32, link: u32, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    let (id, position, found) = read(access, tab, page)?;
    let chosen = link.checked_sub(1).and_then(|at| found.links.get(at as usize)).ok_or_else(|| AgentError::new("no_such_link", format!("Page {page} has {} links.", found.links.len()), exit::NOT_FOUND).then("`recto links --page N` lists them, counted from 1."))?;
    access.with(|s| {
        let id = select(s, &format!("#{}", id.0)).map_err(|_| AgentError::new("no_such_tab", "That tab closed.", exit::NOT_FOUND))?;
        let went = match &chosen.target {
            LinkTarget::Page { page: file_page, within } => {
                let to = s.position_in_same_file(id, position, *file_page).ok_or_else(|| AgentError::new("link_goes_nowhere", "The page this link goes to isn't in the document as it's been edited.", exit::FAILED))?;
                s.jump(id, to, within.map_or(0.0, f64::from))?;
                "page"
            }
            LinkTarget::History { back } => {
                let moved = if *back { s.go_back(id)? } else { s.go_forward(id)? };
                if !moved {
                    return Err(nothing_that_way(*back));
                }
                if *back { "back" } else { "forward" }
            }
            LinkTarget::Web(address) => {
                let mut error = AgentError::new("web_link_needs_user", "This link goes to a web or email address. Only the person using RectoPDF can open one, in the app, after it asks them. No agent command opens it, at any level.", exit::PERMISSION).then("Tell the user where it goes, if it matters: it's under `untrusted`, from the file.");
                error.untrusted = Some(Box::new(json!({ "address": &**address })));
                return Err(error);
            }
            LinkTarget::Refused(why) => return Err(AgentError::new("link_refused", why.message(), exit::FAILED).then("Nothing happened, and nothing will: RectoPDF never does this.")),
            LinkTarget::Nowhere => return Err(AgentError::new("link_goes_nowhere", "This link names a place that isn't in this document.", exit::FAILED)),
        };
        let now = s.tab(id).map_or(0, |tab| tab.page);
        events.push(Event::new("page_changed", json!({ "tab": id.0, "page": now + 1 })));
        let mut answer = where_json(s, id);
        answer["went"] = json!(went);
        Ok(answer)
    })
}

fn nothing_that_way(back: bool) -> AgentError {
    if back {
        AgentError::new("nothing_back", "There's nowhere to go back to in this tab.", exit::FAILED).then("`recto where` says `can_go_back`. Following a link, a contents entry or `recto goto` leaves somewhere to come back to.")
    } else {
        AgentError::new("nothing_forward", "There's nowhere to go forward to in this tab.", exit::FAILED).then("`recto where` says `can_go_forward`. Going back leaves somewhere to go forward to.")
    }
}

/// `recto back` and `recto forward`, as ⌘[ and ⌘] do.
pub fn step<A: SessionAccess>(access: &A, tab: &str, back: bool, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    access.with(|s| {
        let id = select(s, tab)?;
        let moved = if back { s.go_back(id)? } else { s.go_forward(id)? };
        if !moved {
            return Err(nothing_that_way(back));
        }
        let now = s.tab(id).map_or(0, |tab| tab.page);
        events.push(Event::new("page_changed", json!({ "tab": id.0, "page": now + 1 })));
        Ok(where_json(s, id))
    })
}

/// `recto goto label iv`: the page the file calls that, or a plain number no page is called, as the go-to field reads it.
pub fn goto_label<A: SessionAccess>(access: &A, tab: &str, label: &str, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    access.with(|s| {
        let id = select(s, tab)?;
        let page = s.page_for_typed(id, label).ok_or_else(|| AgentError::new("no_such_label", "No page in this document is called that.", exit::NOT_FOUND).then("`recto where` gives the page's `label`. Labels are as the file writes them, such as `iv` or `A-3`, and a plain number is a page."))?;
        s.jump(id, page, 0.0)?;
        events.push(Event::new("page_changed", json!({ "tab": id.0, "page": page + 1 })));
        Ok(where_json(s, id))
    })
}
