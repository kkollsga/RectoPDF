//! **An image picked with a click in read mode, in the window** (owner, 2026-11-16: "when in normal read mode (with left click to select text) clicking a
//! image will show a selection box. Clicking cmd+c will copy lossless to clipboard."). Its rule is `recto_reader::image_pick`, which the page view holds
//! beside the text selection, so whatever ends or changes that lets the image go; this joins it to the look for the image, Copy and Escape.
//!
//! **A click costs a private worker only where an image may be** (the review of `build/image-select`, S1). The page view decides, with no worker of its
//! own: a still single click off any link, with the snapshot tool off, waits for its page's text to say no text that shows is there, and then for the
//! tab's own worker to say an image is (`recto_reader::image_pick`). Only then is the app told ([`recto_reader::engine::EngineEvents::image_wanted`], to
//! the session thread as `CoreCall::ImageLooks`), and [`looks_ready`] sends the person's `images --at x,y` for that point: the one implementation an
//! agent's `recto images --at` is too, found as the snapshot tool's click finds its image. A click on text, on a margin, or the first of a double click
//! starts no worker; a click on an image, at most one.
//!
//! **One private worker at a time, across picks and copies** (S2): the look runs in the snapshot tool's one slot ([`super::snapshot_tool`]), so a newer
//! click's look, or a copy, stops a running look, through the worker's cancel word, and a look waits behind a copy. A click the person has moved on from (a
//! press, a let-go, another tab, an edit of its pages, the snapshot tool turned on) is news from the page view too, and [`looks_ready`] stops its look. Its
//! answer ([`answered`]) is taken only for the click it was sent for, by that click's serial: the last thing asked for wins.
//!
//! **Copy** with an image picked is the snapshot tool's click on it, the same command by the same job ([`super::snapshot_tool::taken`]): its own JPEG
//! bytes with a PNG beside it, or a lossless PNG, the same words, the same "Copy anyway" for a file that asks not to be copied. It isn't made twice here.
//!
//! **What it holds:** nothing of its own. The click is the page view's; the look in the slot is the snapshot tool's.
//!
//! **Agents:** an agent's way to the same image is `recto images --at`, at its level, refused on a document the person opened with a password as every
//! read of one is. An agent's command is never taken for the window's look: only the person's commands are answered here. What the person picked is in
//! `qa state`, as their text selection is, and nowhere else. The clipboard is the person's alone.

use super::HostAction;
use super::person::Person;
use super::row_parts::RowParts;
use super::snapshot_tool::Tool;
use crate::panes::Panes;
use recto_agent::commands::Read;
use recto_agent::commands::export::Images;
use recto_agent::extract::AsSeen;
use recto_agent::{AgentError, Command, TabRef};
use recto_index::select::{PagePoint, Rect};
use recto_reader::Engine;
use recto_session::TabId;
use serde_json::Value;

/// Escape with an image picked lets it go: the window's own id for it.
pub const LET_GO_ID: &str = "image.let_go";
/// The Edit menu's Copy, which takes the image picked when there is one.
const COPY_ID: &str = "edit.copy";
/// What a Mac's Escape key types.
const ESCAPE: &str = "\u{1b}";

/// **A click's look for its image:** the tab, the point on its page (the page counted as the view counts it), the click's serial, and that page as the
/// person saw it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Look {
    /// The pane whose page view the click is in: each has its own clock, so two panes' serials can be the same.
    pane: usize,
    pub(super) tab: u32,
    pub(super) point: PagePoint,
    serial: u32,
    seen: AsSeen,
}

impl Look {
    /// **The person's command for it:** `images --at x,y` on the page clicked, naming its tab and the page as it was seen.
    fn command(self) -> Command {
        Command::from(Images {
            tab: TabRef::Id(TabId(self.tab)),
            page: self.point.page + 1,
            at: Some([self.point.x, self.point.y]),
            as_seen: Some(self.seen),
        })
    }

    /// The same click: its tab and serial.
    pub(super) fn same(&self, other: &Self) -> bool {
        (self.pane, self.tab, self.serial) == (other.pane, other.tab, other.serial)
    }

    /// The click a command of this feature's looks for, read back from it, its tab and point: none for any other, an agent's `images` among them, which
    /// never names the page as seen; an agent's command is never answered here in any case, only the person's are.
    fn asked(command: &Command) -> Option<(u32, PagePoint)> {
        let Command::Read(Read::Images(asked)) = command else { return None };
        let (TabRef::Id(tab), Some([x, y]), Some(_)) = (&asked.tab, asked.at, asked.as_seen) else {
            return None;
        };
        let page = asked.page.checked_sub(1)?;
        Some((tab.0, PagePoint { page, x, y }))
    }

    /// A test's: the `serial`th click, on `tab`, at a point of its own.
    #[cfg(test)]
    pub(super) fn for_test(pane: usize, tab: u32, serial: u32) -> Self {
        Self {
            pane,
            tab,
            point: PagePoint {
                page: 0,
                x: serial as f32,
                y: 1.0,
            },
            serial,
            seen: AsSeen {
                quarter_turns: 0,
                file_page: 0,
                own: true,
            },
        }
    }
}

/// The page view of the look's pane in `engines`, if that click is still the one it looks for.
fn waiting<'a>(engines: &'a [Engine], look: &Look) -> Option<&'a Engine> {
    engines.get(look.pane).filter(|engine| {
        engine
            .selection()
            .image_looking()
            .is_some_and(|(tab, _, serial)| (tab, serial) == (look.tab, look.serial))
    })
}

/// **The page views have news of a click's image** (`CoreCall::ImageLooks`, on the session thread): what each wants looked for now goes to the slot,
/// which starts the newest, stops what's no longer wanted, and says which a newer one took the place of; those, and a click whose page can't be said as
/// seen, are told none was found. A look to start is sent as the `person`'s. Never waits.
pub fn looks_ready(panes: &Panes, engine: &Engine, tool: &Tool, person: Person<'_>) {
    let engines = panes.engines(engine);
    let mut wanted = Vec::new();
    for (pane, engine) in engines.iter().enumerate() {
        let Some((tab, point, serial)) = engine.selection().image_looking() else {
            continue;
        };
        let Some(seen) = engine.points().page_seen(point.page) else {
            engine.selection().image_not_found(tab, serial);
            continue;
        };
        let seen = AsSeen {
            quarter_turns: seen.quarter_turns,
            file_page: seen.file_page,
            own: seen.own,
        };
        wanted.push(Look {
            pane,
            tab,
            point,
            serial,
            seen,
        });
    }
    let (start, replaced) = tool.looks(&wanted);
    let not_found = |look: &Look| {
        if let Some(engine) = waiting(&engines, look) {
            engine.selection().image_not_found(look.tab, look.serial);
        }
    };
    replaced.iter().for_each(not_found);
    if let Some(look) = start
        && !person.submit(look.command())
    {
        tool.look_not_sent();
        not_found(&look);
    }
}

/// The image an answer found: its number and box. None for no image there, or a refusal.
fn found_in(result: &Result<Value, AgentError>) -> Option<(u32, Rect)> {
    let image = result.as_ref().ok()?.get("images")?.get(0)?;
    let number = u32::try_from(image.get("number")?.as_u64()?).ok()?;
    let side = |at: usize| image.get("rect")?.get(at)?.as_f64().map(|value| value as f32);
    let rect = Rect {
        x: side(0)?,
        y: side(1)?,
        width: side(2)?,
        height: side(3)?,
    };
    Some((number, rect))
}

/// **What came of a click's look** ([`super::person`]'s list of answers): taken off the slot if it's the look that ran there, and its image to the page view
/// still waiting for that click, by its serial; then the app looks again at what's wanted, so a look that waited starts. Nothing here takes the session's
/// lock.
pub fn answered(parts: &super::answer_parts::AnswerParts<'_>, ran: &super::person::Ran<'_>) {
    let Some(asked) = Look::asked(ran.command) else { return };
    // Waiting for a read of its file, to run again once it's in: not its answer yet.
    if ran.result.as_ref().is_err_and(recto_agent::later::is_later) {
        return;
    }
    let person = parts.person();
    parts.image_looked(|tool, engines| {
        // A look a copy stopped runs again after it, so its stopped answer says nothing of the click.
        let Some((look, again)) = super::snapshot_tool::look_answered(tool, person, asked) else {
            return;
        };
        let Some(engine) = waiting(engines, &look).filter(|_| !again) else { return };
        match found_in(ran.result) {
            Some((number, rect)) => {
                engine.selection().image_found(look.tab, look.serial, number, rect);
            }
            None => engine.selection().image_not_found(look.tab, look.serial),
        }
    });
}

/// **The person's Copy** (Edit > Copy, ⌘C, the context menu's, and text's Copy anyway): a plain Copy with an image picked in the pane being read is the
/// snapshot tool's click on it, the same command by its one job, with its own words and Copy anyway; text's Copy anyway (`anyway`) is text's alone. Says
/// whether the image took it; false for the text selection's copy.
pub(super) fn copy(person: Person<'_>, anyway: bool) -> bool {
    !anyway && person.copy_image()
}

/// **The image picked in the pane being read, copied:** kept for the snapshot `tool` as its click on that image is kept by a release, and taken as a
/// release's is (`take`, the person's [`super::snapshot_tool::taken`]). False with none picked, or its page not to be said as seen.
pub(super) fn copy_picked(panes: &Panes, engine: &Engine, tool: &Tool, take: impl FnOnce()) -> bool {
    let engine = panes.reading_engine(engine);
    let Some((tab, pick)) = engine.selection().image_picked() else { return false };
    let Some(seen) = engine.points().page_seen(pick.page) else { return false };
    let seen = AsSeen {
        quarter_turns: seen.quarter_turns,
        file_page: seen.file_page,
        own: seen.own,
    };
    tool.take_image(tab, pick.page, seen, [pick.at.0, pick.at.1]);
    take();
    true
}

/// **Every pane's image picked, let go:** Escape, and the snapshot tool turned on. A look under way for a click goes with it: the page views' news stops it.
pub fn let_go(panes: &Panes, engine: &Engine) {
    for engine in panes.engines(engine) {
        engine.selection().let_go_image();
    }
}

/// **Whether a menu id this feature has a say in can be chosen now:** Copy with an image picked in the pane being read (otherwise the text selection's
/// rule), and Escape's id while there's an image picked or a click waiting for one. None for any other id, and nothing's read for one.
pub fn enabled(id: &str, reading: impl FnOnce() -> Engine) -> Option<bool> {
    match id {
        COPY_ID => reading().selection().image_picked().is_some().then_some(true),
        LET_GO_ID => {
            let picking = reading();
            Some(picking.selection().image_picked().is_some() || picking.selection().image_finding())
        }
        _ => None,
    }
}

/// **A key with nothing held** (the bridge's `plain_key_action`): the menus' keys, Capture's Escape first while one waits, and then Escape as this feature's
/// id while there's something of it to let go, and otherwise nobody's, so it goes on to whatever else takes it.
pub fn plain_key(panes: &Panes, engine: &Engine, tool: &Tool, characters: &str) -> Option<&'static str> {
    crate::menus::plain_key_with(characters, tool.is_once())
        .or_else(|| (characters == ESCAPE && enabled(LET_GO_ID, || panes.reading_engine(engine)) == Some(true)).then_some(LET_GO_ID))
}

/// **This feature's menu id** ([`super::menu_rows`]): Escape's, the window's alone.
pub fn row<'a>(parts: &RowParts<'a>, asked: &super::menu_rows::Asked<'_>) -> Option<super::menu_rows::Row<'a>> {
    use super::menu_rows::{Agents, Row};
    if asked.id != LET_GO_ID {
        return None;
    }
    let parts = *parts;
    Some(Row::window(
        "the image the person picked with a click, let go: their own selection in their window, which no agent makes or sees but in `qa state`",
        Agents::Never,
        move || {
            parts.let_go_image();
            HostAction::None
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(x: f32) -> Look {
        Look {
            pane: 0,
            tab: 3,
            point: PagePoint { page: 1, x, y: 20.0 },
            serial: 7,
            seen: AsSeen {
                quarter_turns: 1,
                file_page: 1,
                own: true,
            },
        }
    }

    /// Failure: the look's answer is taken for another command's (an agent's `images`, which names no page as seen, or the snapshot tool's), so an agent's
    /// read picks an image in the person's window; or the look's own command doesn't read back as its click, so its answer is never taken.
    #[test]
    fn only_this_features_look_reads_back_as_one() {
        assert_eq!(Look::asked(&look(5.0).command()), Some((3, look(5.0).point)));
        let agents = Command::from(Images {
            tab: TabRef::Id(TabId(3)),
            page: 2,
            at: Some([5.0, 20.0]),
            as_seen: None,
        });
        assert_eq!(Look::asked(&agents), None);
    }

    /// Failure (rule 6): an answer whose image has no number, a box of the wrong shape or a number past `u32` is taken as some image anyway.
    #[test]
    fn an_answer_that_doesnt_read_finds_no_image() {
        let answer = |image: Value| Ok(serde_json::json!({ "images": [image] }));
        assert_eq!(
            found_in(&answer(serde_json::json!({ "number": 2, "rect": [1.0, 2.0, 3.0, 4.0] }))),
            Some((
                2,
                Rect {
                    x: 1.0,
                    y: 2.0,
                    width: 3.0,
                    height: 4.0
                }
            ))
        );
        for bad in [
            serde_json::json!({ "rect": [1.0, 2.0, 3.0, 4.0] }),
            serde_json::json!({ "number": 2, "rect": [1.0, 2.0] }),
            serde_json::json!({ "number": u64::MAX, "rect": [1.0, 2.0, 3.0, 4.0] }),
            serde_json::json!({ "number": 2, "rect": "everything" }),
        ] {
            assert_eq!(found_in(&answer(bad.clone())), None, "{bad}");
        }
        assert_eq!(found_in(&Ok(serde_json::json!({ "images": [] }))), None, "no image there");
    }
}
