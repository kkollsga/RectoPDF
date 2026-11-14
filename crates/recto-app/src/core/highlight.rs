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
        Some("there's no highlight there")
    } else if error.code == Code::HIGHLIGHT_IN_ADDED_FILE {
        Some("that highlight came with an added PDF, so save first, then it can be removed")
    } else if error.code == Code::CHANGED {
        Some("the pages moved meanwhile, so try again")
    } else {
        person::session_words(error)
    };
    if let Some(words) = words {
        person::say(inner, format!("{doing}: {words}."));
    }
}

/// Sets made lately, by tab and the tick of their list's last change: made again only when it changed (review of R4.3: they were made and compared
/// on every pass, under the session's lock). On the session thread, which makes them; a few, as each pane shows one tab. Each pass's read lets go
/// of a set that isn't its tab's now, a closed or sleeping tab's among them ([`forget_old_sets`]).
const SETS_KEPT: usize = 8;

std::thread_local! {
    static MADE: std::cell::RefCell<Vec<(u32, u64, Arc<TabMarks>)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// **A tab's highlights made as edits, by page position, for its page view to draw:** each highlight on every position its page has; and the file's
/// own it has taken out, for the page view's draws to leave out. None for a tab with none, which is every tab without edits: nothing is made for it.
/// Made once for each change, and the same set given again until the next ([`SETS_KEPT`]).
pub fn marks_of<L: DocLoader>(session: &Session<L>, id: TabId) -> Option<Arc<TabMarks>> {
    let list = session.lists().get(id)?;
    if list.marks().is_empty() {
        return None;
    }
    let tick = list.marks().changes();
    if let Some(made) = MADE.with_borrow(|made| made.iter().find(|(tab, at, _)| (*tab, *at) == (id.0, tick)).map(|(_, _, set)| Arc::clone(set))) {
        return Some(made);
    }
    let made = make_marks(session, id, list);
    MADE.with_borrow_mut(|kept| {
        kept.retain(|(tab, _, _)| *tab != id.0);
        if kept.len() >= SETS_KEPT {
            kept.remove(0);
        }
        kept.push((id.0, tick, Arc::clone(&made)));
    });
    Some(made)
}

/// **Lets go of each set kept that isn't its tab's now** (second review of R4.3, SF5): a closed tab's, a sleeping one's (its list is in its sleep
/// file), one whose highlights changed since. Each pass's read calls it, so nothing of a tab is kept past the pass after it closes. Nothing to do
/// with none kept.
pub fn forget_old_sets<L: DocLoader>(session: &Session<L>) {
    MADE.with_borrow_mut(|kept| {
        kept.retain(|(tab, tick, _)| {
            session
                .lists()
                .get(TabId(*tab))
                .is_some_and(|list| list.marks().changes() == *tick && !list.marks().is_empty())
        });
        if kept.is_empty() {
            *kept = Vec::new();
        }
    });
}

fn make_marks<L: DocLoader>(session: &Session<L>, id: TabId, list: &recto_session::edit::PageEdits) -> Arc<TabMarks> {
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
    let hidden = list.marks().hidden_annots().and_then(|set| Some((session.tabs().path_of(id)?, set)));
    Arc::new(TabMarks { tab: id.0, pages, hidden })
}

/// **The file's highlights a tab has taken out, given to the pane's page view before it shows the tab** (second review of R4.3, S4), so the draws
/// its showing starts leave them out from the first; and a tab it stops showing has its set taken back first. Nothing when it has that set already.
pub fn before_showing(engine: &recto_reader::Engine, synced: &mut MarksSynced, marks: Option<&Arc<TabMarks>>) {
    give_hidden(
        engine,
        synced,
        marks.and_then(|marks| marks.hidden.as_ref().map(|(file, set)| (marks.tab, file, set))),
    );
}

/// What a pane's page view has been given of highlights.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MarksSynced {
    /// The set given and the paper it was given for, or `None` when there's none, or the pane's page view has just shown a tab anew and hasn't been
    /// told.
    given: Option<(Arc<TabMarks>, PaperKind)>,
    /// What the host has may have some on it, which a tab with none has to clear.
    host_has_some: bool,
    /// The set of the file's highlights taken out the page view was last given, by tab and file, and its generation: taken back when the pane shows
    /// another tab or the set's emptied, so the page view keeps none for a tab it doesn't show.
    hidden_given: Option<(u32, std::path::PathBuf, u64)>,
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
    before_showing(engine, synced, marks);
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
                engine.selection().show_marks(
                    &TabMarks {
                        tab: tab.0,
                        pages: Vec::new(),
                        hidden: None,
                    },
                    paper,
                );
                synced.host_has_some = false;
            }
        }
    }
}

/// **Gives the pane's page view the set of the file's highlights its tab has taken out,** when it's another than it was given, and takes back the one
/// it was given when the pane shows another tab or the set's gone. A comparison of a key otherwise.
fn give_hidden(engine: &recto_reader::Engine, synced: &mut MarksSynced, now: Option<(u32, &std::path::PathBuf, &Arc<recto_proto::hidden::HiddenAnnots>)>) {
    let same = match (&synced.hidden_given, now) {
        (Some((tab, file, generation)), Some((now_tab, now_file, set))) => (*tab, file, *generation) == (now_tab, now_file, set.generation),
        (given, now) => given.is_none() && now.is_none(),
    };
    if same {
        return;
    }
    let key = now.map(|(tab, file, set)| (tab, file.clone(), set.generation));
    if let Some((tab, file, _)) = synced.hidden_given.take()
        && now.is_none_or(|(now_tab, now_file, _)| (now_tab, now_file) != (tab, &file))
    {
        engine.selection().show_hidden(tab, &file, None);
    }
    if let Some((tab, file, set)) = now {
        engine.selection().show_hidden(tab, file, Some(Arc::clone(set)));
    }
    synced.hidden_given = key;
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

    /// The tabs a set is kept for, on this thread.
    fn kept_for() -> Vec<u32> {
        MADE.with_borrow(|made| made.iter().map(|(tab, ..)| *tab).collect())
    }

    /// **Failure (second review of R4.3, SF5): a closed tab's highlights stay held** in the sets kept on the session thread, up to eight tabs' worth
    /// whatever was closed, until others push them out. After the close, the next pass's read keeps nothing of it; nor of a tab whose highlights
    /// changed since its set was made.
    #[test]
    fn a_closed_tabs_set_isnt_kept() {
        let file = recto_testpdf::TempPdf::new(
            "kept.pdf",
            &recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["Highlighted words"]))]),
        );
        let mut session = Session::new(recto_agent::DrawableLoader);
        let run = |session: &mut Session<recto_agent::DrawableLoader>, line: &str| {
            recto_agent::run(session, recto_agent::Level::Full, &recto_agent::parse(line).unwrap()).unwrap()
        };
        run(&mut session, &format!(r#"{{"cmd":"open","path":{}}}"#, serde_json::json!(file.0)));
        run(&mut session, r##"{"cmd":"highlight","tab":"active","page":1,"chars":"0-5"}"##);
        let id = session.tabs().active().expect("a tab");
        assert!(marks_of(&session, id).is_some());
        assert_eq!(kept_for(), vec![id.0]);
        forget_old_sets(&session);
        assert_eq!(kept_for(), vec![id.0], "still its set");
        run(&mut session, r##"{"cmd":"discard","tab":"active"}"##);
        run(&mut session, r##"{"cmd":"close","tab":"active"}"##);
        forget_old_sets(&session);
        assert!(kept_for().is_empty(), "nothing of the closed tab");
    }
}
