//! **Links, Back and Forward, and pages by their labels,** for an agent: `recto links`, `recto follow`, `recto back`, `recto forward` and `recto goto label`.
//!
//! An agent sees where a link goes and can follow one within the document, as a click does. It never opens a web address: only the person can, in the app, after the app has asked them and shown the real address. A link's address comes from the file, so it goes under `untrusted`, and nothing about a link is ever run.

use crate::Code;
use crate::TabRef;
use crate::{AgentError, Event, SessionAccess, exit, locate, page_now, select, where_json};
use recto_core::nav::{LinkTarget, PageLinks};
use recto_session::{Author, DocLoader, Session, TabId};
use serde_json::{Value, json};

/// What a link does, in an agent's terms: where it goes now, with edits counted, or why it doesn't.
pub fn link_json<L: DocLoader>(session: &Session<L>, id: TabId, from: u32, index: usize, rect: [f32; 4], target: &LinkTarget) -> Value {
    let rect = [rect[0], rect[1], rect[2] - rect[0], rect[3] - rect[1]];
    let mut out = json!({ "link": index + 1, "rect": rect });
    match target {
        LinkTarget::Page { page, within } => match session.pages().position_in_same_file(id, from, *page) {
            Some(position) => {
                out["goes_to"] = json!("page");
                out["page"] = json!(position + 1);
                out["within"] = json!(within);
                if let Some(label) = session.pages().label_differs(id, position) {
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

/// One page's links, read off the session's lock: a file on a slow disk holds up only this. With the page's unsaved quarter turns.
fn read<A: SessionAccess>(access: &A, tab: &TabRef, page: u32) -> Result<(TabId, u32, PageLinks), AgentError> {
    read_turned(access, tab, page).map(|(id, position, links, _)| (id, position, links))
}

fn read_turned<A: SessionAccess>(access: &A, tab: &TabRef, page: u32) -> Result<(TabId, u32, PageLinks, u8), AgentError> {
    let (id, position, source) = locate(access, tab, page)?;
    let read = recto_core::nav::page_links(&source.path, &[source.page]).map_err(AgentError::from)?;
    let links = read.into_iter().next().map(|(_, links)| links).unwrap_or_default();
    Ok((id, position, links, source.quarter_turns % 4))
}

/// A box `[x0, y0, x1, y1]` on a page `size` wide and high as the file shows it, from its top left, where it is once the page is turned clockwise by quarter turns, as `[x, y, width, height]`.
fn turned_box([x0, y0, x1, y1]: [f32; 4], (width, height): (f32, f32), turns: u8) -> [f32; 4] {
    let turn = |x: f32, y: f32| match turns % 4 {
        1 => (height - y, x),
        2 => (width - x, height - y),
        3 => (y, width - x),
        _ => (x, y),
    };
    let ((ax, ay), (bx, by)) = (turn(x0, y0), turn(x1, y1));
    [ax.min(bx), ay.min(by), (ax - bx).abs(), (ay - by).abs()]
}

/// `recto links`: what's on a page, and where each goes. On a page with an unsaved turn each link also has `shown_rect`, its box on the page as it's seen turned (PLAN, links follow-ups: agents got the file's box for a page the reader sees turned).
pub fn links<A: SessionAccess>(access: &A, tab: &TabRef, page: u32) -> Result<Value, AgentError> {
    let (id, position, found, turns) = read_turned(access, tab, page)?;
    let size = found.frame.shown_size().filter(|_| turns != 0);
    access.with(|s| {
        let links: Vec<Value> = found
            .links
            .iter()
            .enumerate()
            .map(|(index, link)| {
                let mut out = link_json(s, id, position, index, link.rect, &link.target);
                if let Some(size) = size {
                    out["shown_rect"] = json!(turned_box(link.rect, size, turns));
                }
                out
            })
            .collect();
        let mut answer = json!({ "tab": id.0, "page": position + 1, "untrusted": { "label": s.pages().label_differs(id, position) }, "links": links, "truncated": found.truncated, "note": "`rect` is x, y, width, height in points on the page as the file shows it, from its top left. `recto follow` follows one within the document." });
        if size.is_some() {
            answer["quarter_turns"] = json!(turns);
            answer["note"] = json!("`rect` is x, y, width, height in points on the page as the file shows it, from its top left, and `shown_rect` the same box on the page as it's seen with its unsaved turn of `quarter_turns` clockwise. `recto follow` follows one within the document.");
        }
        Ok(answer)
    })
}

/// `recto follow`: what a click on a link does, within the document. A web address is the person's to open, and never an agent's.
pub fn follow<A: SessionAccess>(access: &A, by: Author, tab: &TabRef, page: u32, link: u32, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    let (id, position, found) = read(access, tab, page)?;
    // After the read, which can take a while on a slow disk, so the place taken in is the one showing as it goes.
    access.take_places();
    let chosen = link.checked_sub(1).and_then(|at| found.links.get(at as usize)).ok_or_else(|| {
        AgentError::new(Code::NO_SUCH_LINK, format!("Page {page} has {} links.", found.links.len()), exit::NOT_FOUND)
            .then("`recto links --page N` lists them, counted from 1.")
    })?;
    access.with(|s| {
        let id = select(s, &TabRef::Id(id)).map_err(|_| AgentError::new(Code::NO_SUCH_TAB, "That tab closed.", exit::NOT_FOUND))?;
        let went = match &chosen.target {
            LinkTarget::Page { page: file_page, within } => {
                let to = s.pages().position_in_same_file(id, position, *file_page).ok_or_else(|| AgentError::new(Code::LINK_GOES_NOWHERE, "The page this link goes to isn't in the document as it's been edited.", exit::FAILED))?;
                s.jump_as(id, to, within.map_or(0.0, f64::from), by)?;
                "page"
            }
            LinkTarget::History { back } => {
                let moved = s.step_as(id, *back, by)?;
                if !moved {
                    return Err(nothing_that_way(*back));
                }
                if *back { "back" } else { "forward" }
            }
            LinkTarget::Web(address) => {
                let mut error = AgentError::new(Code::WEB_LINK_NEEDS_USER, "This link goes to a web or email address. Only the person using RectoPDF can open one, in the app, after it asks them. No agent command opens it, at any level.", exit::PERMISSION).then("Tell the user where it goes, if it matters: it's under `untrusted`, from the file.");
                error.untrusted = Some(Box::new(json!({ "address": &**address })));
                return Err(error);
            }
            LinkTarget::Refused(why) => return Err(AgentError::new(Code::LINK_REFUSED, why.message(), exit::FAILED).then("Nothing happened, and nothing will: RectoPDF never does this.")),
            LinkTarget::Nowhere => return Err(AgentError::new(Code::LINK_GOES_NOWHERE, "This link names a place that isn't in this document.", exit::FAILED)),
        };
        events.push(page_now(s, id));
        let mut answer = where_json(s, id);
        answer["went"] = json!(went);
        Ok(answer)
    })
}

fn nothing_that_way(back: bool) -> AgentError {
    if back {
        AgentError::new(Code::NOTHING_BACK, "There's nowhere to go back to in this tab.", exit::FAILED)
            .then("`recto where` says `can_go_back`. Following a link, a contents entry or `recto goto` leaves somewhere to come back to.")
    } else {
        AgentError::new(Code::NOTHING_FORWARD, "There's nowhere to go forward to in this tab.", exit::FAILED)
            .then("`recto where` says `can_go_forward`. Going back leaves somewhere to go forward to.")
    }
}

/// `recto back` and `recto forward`, as ⌘[ and ⌘] do.
pub fn step<A: SessionAccess>(access: &A, by: Author, tab: &TabRef, back: bool, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    access.with(|s| {
        let id = select(s, tab)?;
        let moved = s.step_as(id, back, by)?;
        if !moved {
            return Err(nothing_that_way(back));
        }
        events.push(page_now(s, id));
        Ok(where_json(s, id))
    })
}

/// `recto goto label iv`: the page the file calls that, or a plain number no page is called, as the go-to field reads it.
pub fn goto_label<A: SessionAccess>(access: &A, by: Author, tab: &TabRef, label: &str, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    access.with(|s| {
        let id = select(s, tab)?;
        let page = s.pages().for_typed(id, label).ok_or_else(|| {
            AgentError::new(Code::NO_SUCH_LABEL, "No page in this document is called that.", exit::NOT_FOUND)
                .then("`recto where` gives the page's `label`. Labels are as the file writes them, such as `iv` or `A-3`, and a plain number is a page.")
        })?;
        s.jump_as(id, page, 0.0, by)?;
        events.push(page_now(s, id));
        Ok(where_json(s, id))
    })
}

#[cfg(test)]
mod tests {
    use super::turned_box;

    /// Failure: a link's box on a page with an unsaved turn is given where the file has it, so an agent looking at the page as the reader sees it points at the wrong place; or a turn lands the box off the turned page. A box near the top left of a 600 x 800 page, turned each way.
    #[test]
    fn a_links_box_turns_with_its_page() {
        let link = [10.0, 20.0, 110.0, 50.0];
        let size = (600.0, 800.0);
        assert_eq!(turned_box(link, size, 0), [10.0, 20.0, 100.0, 30.0]);
        // A quarter turn clockwise: the page's left edge becomes its top, so the box is at the top right of the 800-wide page.
        assert_eq!(turned_box(link, size, 1), [750.0, 10.0, 30.0, 100.0]);
        assert_eq!(turned_box(link, size, 2), [490.0, 750.0, 100.0, 30.0]);
        assert_eq!(turned_box(link, size, 3), [20.0, 490.0, 30.0, 100.0]);
        // Four quarter turns are none.
        assert_eq!(turned_box(link, size, 4), turned_box(link, size, 0));
    }
}
