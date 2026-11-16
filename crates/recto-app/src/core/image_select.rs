//! **An image picked with a click in read mode, in the window** (owner, 2026-11-16: "when in normal read mode (with left click to select text) clicking a
//! image will show a selection box. Clicking cmd+c will copy lossless to clipboard."). Its rule is `recto_reader::image_pick`, which the page view holds
//! beside the text selection, so whatever ends or changes that lets the image go; this joins it to the click, the look for the image, Copy and Escape.
//!
//! **A still single click** off any link, with the snapshot tool off and not on text that shows, wants the image under it. **Finding it is a read**
//! through a worker (0.8 s on the 346 MiB USGS map), so the click never waits: [`clicked`] sends the person's `images --at x,y` for that point, the one
//! implementation an agent's `recto images --at` is too, found as the snapshot tool's click finds its image. **One look at a time:** a click while one is
//! under way waits in its place, the newest only, so ten clicks on a map are one worker at a time. The look's answer ([`answered`]) is taken by the page view
//! only if that click is still the one waiting, which a press since, another click, another tab or an edit has ended: the last thing asked for wins.
//!
//! **Copy** with an image picked is the snapshot tool's click on it, the same command by the same job ([`super::snapshot_tool::take_image`]): its own JPEG
//! bytes with a PNG beside it, or a lossless PNG, the same words, the same "Copy anyway" for a file that asks not to be copied. It isn't made twice here.
//!
//! **What it holds:** whether a look is under way, and the newest click waiting for it. At rest, nothing: no thread, worker, timer or buffer.
//!
//! **Agents:** an agent's way to the same image is `recto images --at`, at its level, refused on a document the person opened with a password as every
//! read of one is. What the person picked is in `qa state`, as their text selection is, and nowhere else. The clipboard is the person's alone.

use super::person::Person;
use super::row_parts::RowParts;
use super::{HostAction, lock};
use crate::panes::Panes;
use recto_agent::commands::Read;
use recto_agent::commands::export::Images;
use recto_agent::extract::AsSeen;
use recto_agent::{AgentError, Command, TabRef};
use recto_index::select::{PagePoint, Rect};
use recto_reader::Engine;
use recto_session::TabId;
use serde_json::Value;
use std::sync::Mutex;

/// Escape with an image picked lets it go: the window's own id for it.
pub const LET_GO_ID: &str = "image.let_go";
/// The Edit menu's Copy, which takes the image picked when there is one.
const COPY_ID: &str = "edit.copy";
/// What a Mac's Escape key types.
const ESCAPE: &str = "\u{1b}";

/// A click's look for its image: the tab, the point on its page (the page counted as the view counts it), and that page as the person saw it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Look {
    tab: u32,
    point: PagePoint,
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

    /// The look a command of this feature's is, read back from it: none for any other, an agent's `images` among them, which never names the page as seen.
    fn of(command: &Command) -> Option<Self> {
        let Command::Read(Read::Images(asked)) = command else { return None };
        let (TabRef::Id(tab), Some([x, y]), Some(seen)) = (&asked.tab, asked.at, asked.as_seen) else {
            return None;
        };
        Some(Self {
            tab: tab.0,
            point: PagePoint {
                page: asked.page.checked_sub(1)?,
                x,
                y,
            },
            seen,
        })
    }
}

/// **One look at a time:** whether one is under way, and the newest click waiting for it.
#[derive(Default)]
struct Slot {
    running: bool,
    waiting: Option<Look>,
}

/// **The window's side of an image picked with a click.** See the module's notes.
#[derive(Default)]
pub struct ImageSelect {
    slot: Mutex<Slot>,
}

// size: one for the app, never per tab or page: a look under way and the newest click waiting (40 bytes on 2026-11-16, `build/image-select`).
const _: () = assert!(size_of::<ImageSelect>() <= 40, "the window's side of an image picked grew");

impl ImageSelect {
    /// A look to make: now, if none is under way; otherwise it waits in the place of any waiting, and nothing starts.
    fn offer(&self, look: Look) -> Option<Look> {
        let mut slot = lock(&self.slot);
        if slot.running {
            slot.waiting = Some(look);
            return None;
        }
        slot.running = true;
        Some(look)
    }

    /// The look under way has landed: the one waiting goes next, if there is one.
    fn landed(&self) -> Option<Look> {
        let mut slot = lock(&self.slot);
        slot.running = false;
        let next = slot.waiting.take()?;
        slot.running = true;
        Some(next)
    }

    /// A look that couldn't be sent, as the app hands off: nothing's under way, and nothing waits.
    fn not_sent(&self) {
        *lock(&self.slot) = Slot::default();
    }

    /// A look is under way or waits its turn. A look under way is a person's command running, which a settle and a hand-off wait for already.
    pub fn busy(&self) -> bool {
        let slot = lock(&self.slot);
        slot.running || slot.waiting.is_some()
    }
}

/// **The mouse came up on a page in `pane`** ([`super::controls::UiAction::LinkClicked`]): if it was a still click that wants an image, its look is sent
/// as the `person`'s, or waits its turn. Never waits.
pub(super) fn clicked(panes: &Panes, engine: &Engine, select: &ImageSelect, person: Person<'_>, pane: usize) {
    let Some(engine) = panes.engine(engine, pane) else { return };
    let Some((tab, point)) = engine.selection().image_wanted() else { return };
    let Some(seen) = engine.points().page_seen(point.page) else {
        engine.selection().image_not_found(tab, point);
        return;
    };
    let look = Look {
        tab,
        point,
        seen: AsSeen {
            quarter_turns: seen.quarter_turns,
            file_page: seen.file_page,
            own: seen.own,
        },
    };
    if let Some(look) = select.offer(look)
        && !person.submit(look.command())
    {
        select.not_sent();
        engine.selection().image_not_found(tab, point);
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

/// **What came of a click's look** ([`super::person`]'s list of answers): the image to the page view that's waiting for it, picked if that click is still
/// the last thing asked for there, and the look waiting next sent. Nothing here takes the session's lock.
pub fn answered(parts: &super::answer_parts::AnswerParts<'_>, ran: &super::person::Ran<'_>) {
    let Some(look) = Look::of(ran.command) else { return };
    // Waiting for a read of its file, to run again once it's in: not its answer yet.
    if ran.result.as_ref().is_err_and(recto_agent::later::is_later) {
        return;
    }
    let next = parts.image_looked(|select, engines| {
        if let Some(engine) = engines.iter().find(|engine| engine.selection().image_wanted() == Some((look.tab, look.point))) {
            match found_in(ran.result) {
                Some((number, rect)) => {
                    engine.selection().image_found(look.tab, look.point, number, rect);
                }
                None => engine.selection().image_not_found(look.tab, look.point),
            }
        }
        select.landed()
    });
    if let Some(next) = next
        && !parts.person().submit(next.command())
    {
        parts.image_looked(|select, _| select.not_sent());
    }
}

/// **The person's Copy, with an image picked in the pane being read:** kept for the snapshot tool as its click on that image is kept by a release, for the
/// person's run that takes it ([`super::person::Person::copy_image`]), so it's that tool's command by its one job. False with none picked, for the text
/// selection's copy.
pub(super) fn copy(panes: &Panes, engine: &Engine, tool: &super::snapshot_tool::Tool) -> bool {
    let engine = panes.reading_engine(engine);
    let Some((tab, pick)) = engine.selection().image_picked() else { return false };
    let Some(seen) = engine.points().page_seen(pick.page) else { return false };
    let seen = AsSeen {
        quarter_turns: seen.quarter_turns,
        file_page: seen.file_page,
        own: seen.own,
    };
    tool.take_image(tab, pick.page, seen, [pick.at.0, pick.at.1]);
    true
}

/// **Every pane's image picked, let go:** Escape, and the snapshot tool turned on.
pub fn let_go(panes: &Panes, engine: &Engine) {
    for engine in panes.engines(engine) {
        engine.selection().let_go_image();
    }
}

/// **Whether a menu id this feature has a say in can be chosen now:** Copy with an image picked in the pane being read (otherwise the text selection's
/// rule), and Escape's id while there's an image picked or a click's look under way. None for any other id, and nothing's read for one.
pub fn enabled(id: &str, reading: impl FnOnce() -> Engine) -> Option<bool> {
    match id {
        COPY_ID => reading().selection().image_picked().is_some().then_some(true),
        LET_GO_ID => {
            let picking = reading();
            Some(picking.selection().image_picked().is_some() || picking.selection().image_wanted().is_some())
        }
        _ => None,
    }
}

/// **A key with nothing held** (the bridge's `plain_key_action`): the menus' keys, and Escape, which is this feature's id while there's something of it to
/// let go, and otherwise nobody's, so it goes on to whatever else takes it.
pub fn plain_key(panes: &Panes, engine: &Engine, characters: &str) -> Option<&'static str> {
    crate::menus::plain_key(characters)
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
            tab: 3,
            point: PagePoint { page: 1, x, y: 20.0 },
            seen: AsSeen {
                quarter_turns: 1,
                file_page: 1,
                own: true,
            },
        }
    }

    /// Failure: each click on a heavy page starts a worker of its own, so ten clicks on the USGS map are ten workers of 350 MiB at once (the snapshot tool's
    /// review, 2026-11-08); or the look sent after a burst isn't the newest; or a look that couldn't be sent leaves the feature waiting for good.
    #[test]
    fn looks_go_one_at_a_time_and_only_the_newest_waits() {
        let select = ImageSelect::default();
        assert_eq!(select.offer(look(1.0)), Some(look(1.0)), "nothing under way, so it goes");
        for x in 2..=10 {
            assert_eq!(select.offer(look(x as f32)), None, "one under way, so the rest wait");
        }
        assert!(select.busy());
        assert_eq!(select.landed(), Some(look(10.0)), "only the newest");
        assert_eq!(select.landed(), None, "and nothing after it");
        assert!(!select.busy());
        assert_eq!(select.offer(look(11.0)), Some(look(11.0)));
        select.not_sent();
        assert!(!select.busy(), "not sent, nothing's under way");
    }

    /// Failure: the look's answer is taken for another command's (an agent's `images`, which names no page as seen, or the snapshot tool's), so an agent's
    /// read picks an image in the person's window; or the look's own command doesn't read back as itself, so its answer is never taken.
    #[test]
    fn only_this_features_look_reads_back_as_one() {
        assert_eq!(Look::of(&look(5.0).command()), Some(look(5.0)));
        let agents = Command::from(Images {
            tab: TabRef::Id(TabId(3)),
            page: 2,
            at: Some([5.0, 20.0]),
            as_seen: None,
        });
        assert_eq!(Look::of(&agents), None);
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
