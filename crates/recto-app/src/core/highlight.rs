//! **The window's highlights** (R4.3): Mark Up > Highlight (⌃⌘H) and Remove Highlight, which send the person's selection as `recto highlight` and
//! `recto highlight remove`, the commands an agent sends, run for the person by the one executor; the words a refusal is said to the person in; and the
//! tab's highlights made as edits, given to the page view to draw.
//!
//! What decides is here or in `recto_agent::highlighting`; the host only draws.

use crate::core::menu_rows::{Agents, Asked, Row};
use crate::core::{Inner, person};
use crate::menus::HostAction;
use crate::menus::{MenuEntry, MenuModel};
use recto_agent::commands::Edit;
use recto_agent::commands::highlight::{Highlight, HighlightRemove, RemoveWhat};
use recto_agent::{Code, Command, TabRef};
use recto_index::select::{Range, Rect};
use recto_reader::PaperKind;
use recto_reader::tiles::TabMarks;
use recto_session::{DocLoader, Session, TabId};
use std::sync::Arc;

/// Mark Up > Highlight's id. Its key is ⌃⌘H, as Preview's (coordinator's default, Q2, 2026-11-14).
pub const HIGHLIGHT_ID: &str = "markup.highlight";
/// Mark Up > Remove Highlight's id.
pub const REMOVE_ID: &str = "markup.remove_highlight";

/// Whether a menu id is one of these, for its enable rule and QA's `menu`.
pub fn is_menu_id(id: &str) -> bool {
    id == HIGHLIGHT_ID || id == REMOVE_ID
}

/// The Mark Up menu (SPEC section 4's menu map), with only what's built: Highlight and Remove Highlight, each enabled with a selection.
pub fn mark_up_menu() -> MenuModel {
    let item = |id: &str, title: &str, key: &str, with_control: bool| MenuEntry {
        id: id.into(),
        title: title.into(),
        key: key.into(),
        with_control,
        checked: false,
        hidden: false,
        system_action: String::new(),
        parent: String::new(),
    };
    MenuModel {
        title: "Mark Up".into(),
        entries: vec![item(HIGHLIGHT_ID, "Highlight", "h", true), item(REMOVE_ID, "Remove Highlight", "", false)],
    }
}

/// `recto highlight` of what's selected on a tab: the selection's two ends, as `--page`, `--to-page` and `--chars` take them.
pub fn highlight_of(tab: u32, range: Range) -> Command {
    let (page, to_page, chars) = pick_of(range);
    Command::Edit(Edit::Highlight(Highlight {
        tab: TabRef::Id(TabId(tab)),
        page: Some(page),
        to_page,
        chars: Some(chars),
        at: None,
        to: None,
        unit: None,
    }))
}

/// `recto highlight remove` of what's selected: every unsaved highlight its text touches.
pub fn remove_of(tab: u32, range: Range) -> Command {
    let (page, to_page, chars) = pick_of(range);
    Command::Edit(Edit::HighlightRemove(HighlightRemove {
        tab: TabRef::Id(TabId(tab)),
        what: RemoveWhat::Pick {
            page: Some(page),
            to_page,
            chars: Some(chars),
            at: None,
            to: None,
        },
    }))
}

/// A selection's ends as a pick: its first page from 1, its last if that's later, and `START-END` in characters, the start on the first page and the
/// end on the last, as `recto select` reads them.
fn pick_of(range: Range) -> (u32, Option<u32>, String) {
    let (start, end) = (range.start, range.end);
    let to_page = (end.page != start.page).then_some(end.page + 1);
    (start.page + 1, to_page, format!("{}-{}", start.at, end.at))
}

/// **The Mark Up menu's ids** ([`super::menu_rows`]): each takes the person's selection, which is theirs alone, and sends it as the command an
/// agent has for itself, `recto highlight` or `recto highlight remove`, run as the person's. Nothing selected sends nothing.
pub fn row<'a>(inner: &'a Arc<Inner>, asked: &Asked<'_>) -> Option<Row<'a>> {
    let make: fn(u32, Range) -> Command = match asked.id {
        HIGHLIGHT_ID => highlight_of,
        REMOVE_ID => remove_of,
        _ => return None,
    };
    let instead = if asked.id == HIGHLIGHT_ID {
        "`recto highlight`"
    } else {
        "`recto highlight remove`"
    };
    Some(Row::window(
        "the person's selection, which only they have; an agent picks with the command's own arguments",
        Agents::ItsOwn(instead),
        move || {
            let selected = inner.panes.reading_engine(&inner.engine).selection().hot();
            if let Some((tab, Some(range))) = selected {
                person::submit(inner, make(tab, range));
            }
            HostAction::None
        },
    ))
}

/// **How a person's highlight went, in words for them:** a refusal said by the page, as an agent's words are never shown to the person.
pub fn answered(inner: &Arc<Inner>, ran: &person::Ran<'_>) {
    let doing = match ran.command {
        Command::Edit(Edit::Highlight(_)) => "Couldn't highlight",
        Command::Edit(Edit::HighlightRemove(_)) => "Couldn't remove the highlight",
        _ => return,
    };
    let (Err(_), Some(error)) = (ran.result, ran.refused()) else { return };
    let words = if error.code == Code::NO_TEXT_THERE {
        Some("there's no text selected there")
    } else if error.code == Code::NO_SUCH_HIGHLIGHT {
        Some("there's no highlight made since the last save there")
    } else if error.code == Code::HIGHLIGHT_IN_FILE {
        Some("that highlight is part of the file, and taking those out isn't built yet")
    } else if error.code == Code::CHANGED {
        Some("the pages moved meanwhile, so try again")
    } else {
        person::session_words(error)
    };
    if let Some(words) = words {
        person::say(inner, format!("{doing}: {words}."));
    }
}

/// **A tab's highlights made as edits, by page position, for its page view to draw:** each highlight on every position its page has. None for a tab
/// with none, which is every tab without edits: nothing is made for it.
pub fn marks_of<L: DocLoader>(session: &Session<L>, id: TabId) -> Option<Arc<TabMarks>> {
    let list = session.lists().get(id)?;
    if list.marks().is_empty() {
        return None;
    }
    let mut pages: Vec<(u32, Vec<Rect>)> = Vec::new();
    for (position, mark) in list.marks().on_positions(list.slots()) {
        let rects = mark.rects.iter().map(|rect| Rect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        });
        match pages.last_mut() {
            Some((page, held)) if *page == position => held.extend(rects),
            _ => pages.push((position, rects.collect())),
        }
    }
    Some(Arc::new(TabMarks { tab: id.0, pages }))
}

/// What a pane's page view has been given of highlights.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MarksSynced {
    /// The set given and the paper it was given for, or `None` when there's none, or the pane's page view has just shown a tab anew and hasn't been
    /// told.
    given: Option<(Arc<TabMarks>, PaperKind)>,
    /// What the host has may have some on it, which a tab with none has to clear.
    host_has_some: bool,
}

impl MarksSynced {
    /// The page view has shown a tab anew: a set it has is given again.
    pub fn shown_anew(&mut self) {
        self.given = None;
    }
}

/// **Gives a pane's page view the tab's highlights** when they've changed, the paper has, or it has shown the tab anew; and clears what the host has
/// when the tab has none and some may be there. Nothing otherwise, so a tab without highlights, and every pass with nothing new, costs a comparison
/// and no op.
pub fn give(engine: &recto_reader::Engine, synced: &mut MarksSynced, tab: TabId, marks: Option<&Arc<TabMarks>>, paper: PaperKind) {
    match marks {
        Some(marks) => {
            let now = Some((Arc::clone(marks), paper));
            if synced.given != now {
                engine.selection().show_marks(marks, paper);
                synced.given = now;
                synced.host_has_some = !marks.pages.is_empty();
            }
        }
        None => {
            synced.given = None;
            if synced.host_has_some {
                engine.selection().show_marks(&TabMarks { tab: tab.0, pages: Vec::new() }, paper);
                synced.host_has_some = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recto_index::select::Place;

    /// Failure: the person's selection is sent as another pick than it is: off by a page, its end on the wrong page, or a one-page selection given a
    /// `--to-page`, which `recto select` reads as a pick over pages.
    #[test]
    fn a_selection_is_sent_as_the_pick_select_reads() {
        let one = Range {
            start: Place { page: 2, at: 10 },
            end: Place { page: 2, at: 40 },
        };
        assert_eq!(pick_of(one), (3, None, "10-40".into()));
        let over = Range {
            start: Place { page: 2, at: 900 },
            end: Place { page: 4, at: 12 },
        };
        assert_eq!(pick_of(over), (3, Some(5), "900-12".into()));
        let Command::Edit(Edit::Highlight(sent)) = highlight_of(7, over) else {
            panic!()
        };
        assert_eq!((sent.tab, sent.page, sent.to_page), (TabRef::Id(TabId(7)), Some(3), Some(5)));
    }

    /// Failure: Mark Up's items aren't where the menu map says, Highlight hasn't Preview's key, or an id isn't one this module answers for.
    #[test]
    fn mark_up_has_highlight_on_preview_key() {
        let menu = mark_up_menu();
        assert_eq!(menu.title, "Mark Up");
        let highlight = &menu.entries[0];
        assert_eq!(
            (highlight.id.as_str(), highlight.key.as_str(), highlight.with_control),
            (HIGHLIGHT_ID, "h", true)
        );
        assert!(menu.entries.iter().all(|entry| is_menu_id(&entry.id)));
        assert!(!is_menu_id("tools.snapshot"));
    }
}
