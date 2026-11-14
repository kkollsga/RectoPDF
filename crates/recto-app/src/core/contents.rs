//! **The sidebar's contents, as the person uses them** (step R2.3, C5f): a section's chevron is `contents fold`, and the note over a tab's contents is
//! `contents show own|generated` or `contents stop`, the commands an agent sends, run for the person. Which entry a chevron is on, and what the note offers,
//! are what the session published.

use recto_agent::commands::contents::{ContentsFold, ContentsShow, ContentsStop, Which};
use recto_agent::{Command, TabRef};
use recto_session::{ContentsAction, TabId};

/// **Some of the published contents as the rows the host draws, with the rail's marks** (moved from the bridge, which only converts now): rows `first` up to
/// `count` of them, at most [`super::MAX_CONTENTS_FETCH`] and never past the list. `rail_filled` is how many entries the rail has filled, from the top; the
/// first and last entries draw its ends; `current` is the entry being read. A list is hostile in length: nothing here is sized by what the caller asks for or
/// the file claims, only by the rows actually handed back, which are at most the cap.
pub fn rail_rows(contents: &[crate::ContentsEntry], rail_filled: u32, current: Option<u32>, first: u32, count: u32) -> Vec<crate::SidebarRow> {
    let total = u32::try_from(contents.len()).unwrap_or(u32::MAX);
    let end = first.saturating_add(count.min(super::MAX_CONTENTS_FETCH)).min(total);
    (first..end)
        .filter_map(|index| {
            let entry = contents.get(index as usize)?;
            Some(crate::SidebarRow {
                kind: crate::RowKind::Contents,
                tab: entry.tab,
                text: entry.text.to_string(),
                page: entry.page,
                depth: entry.depth,
                current: current == Some(index),
                rail_filled: index < rail_filled,
                rail_first: index == 0,
                rail_last: index + 1 == total,
                notice: None,
                action: None,
                expandable: entry.expandable,
                expanded: entry.expanded,
                in_comparison: false,
            })
        })
        .collect()
}

/// A tap on the note over a tab's contents: Generate and Show generated are both `contents show generated`, which makes them if they aren't made.
pub fn note_command(tab: TabId, action: ContentsAction) -> Command {
    let tab = TabRef::Id(tab);
    let show = |which: Which| ContentsShow { which, tab: tab.clone() }.into();
    match action {
        ContentsAction::Generate | ContentsAction::ShowGenerated => show(Which::Generated),
        ContentsAction::ShowOwn => show(Which::Own),
        ContentsAction::Stop => ContentsStop { tab: tab.clone() }.into(),
    }
}

/// A contents row's chevron: `contents fold`, its section folded or opened as it is when it runs, by the row's place in what the session published. A row
/// with nothing under it has no chevron, and does nothing.
pub fn chevron(contents: &[crate::ContentsEntry], first: u32) -> Option<Command> {
    let entry = contents.get(first as usize).map(|entry| (entry.tab, entry.entry, entry.expandable));
    let Some((tab, entry, true)) = entry else { return None };
    Some(
        ContentsFold {
            entry,
            tab: TabRef::Id(TabId(tab)),
        }
        .into(),
    )
}

/// Why `qa click-row fold:N` has nothing to click, in words, or `None` when row `row` (counted down the sidebar, tabs and then the active tab's contents, as
/// `qa click-row` counts) is a contents row with sections under it. A person's click on a row that isn't one does nothing, which is right for a person; a QA
/// verb that did nothing and said it was done hid a wrong number (the entry number of `contents fold` taken for a row).
pub fn fold_row_refusal(contents: &[crate::ContentsEntry], active: Option<u32>, toc_open: bool, row: u32) -> Option<String> {
    let first = active.filter(|_| toc_open).map(|active| active + 1);
    let Some(first) = first.filter(|first| row >= *first) else {
        return Some(format!(
            "Row {row} isn't a contents row. Contents rows come after the active tab's row, and only while its contents are open."
        ));
    };
    match contents.get((row - first) as usize) {
        None => Some(format!(
            "Row {row} is past the last contents row ({}). `fold:N` counts rows down the sidebar; `recto contents fold ENTRY` takes the entry's number.",
            first + u32::try_from(contents.len()).unwrap_or(u32::MAX).saturating_sub(1)
        )),
        // The chevron's own rule, so the verb and the window agree on what folds.
        Some(_) if chevron(contents, row - first).is_none() => Some(format!("Row {row} has no sections under it, so it has nothing to fold or open.")),
        Some(_) => None,
    }
}

#[cfg(test)]
mod rail_tests {
    use super::*;
    use crate::ContentsEntry;

    fn list(depths: &[u8]) -> Vec<ContentsEntry> {
        depths
            .iter()
            .enumerate()
            .map(|(at, depth)| ContentsEntry {
                tab: 1,
                text: format!("Entry {at}").into(),
                page: Some(at as u32),
                depth: *depth,
                entry: at as u32,
                top: 0,
                expandable: false,
                expanded: false,
            })
            .collect()
    }

    /// What the host is told per row: (current, filled, first, last).
    fn marks(rows: &[crate::SidebarRow]) -> Vec<(bool, bool, bool, bool)> {
        rows.iter().map(|row| (row.current, row.rail_filled, row.rail_first, row.rail_last)).collect()
    }

    /// **Failure: the rail's marks change when the code that works them out moves** (they were the bridge's): a list's ends don't draw as ends, the fill runs one
    /// entry too far or short, the current entry is marked on another row, or a fetch past the list's end hands back rows or a panic. Expected values are written
    /// out from the rules the bridge had (index < filled, first, last, current), for the lists the guard names: empty, one, nested, entries on one page, an
    /// entry past the last page, and a window that begins past the end.
    #[test]
    fn the_rail_marks_what_the_bridge_marked() {
        assert_eq!(rail_rows(&[], 0, None, 0, 50), vec![], "an empty list has no rows");
        let one = list(&[0]);
        assert_eq!(
            marks(&rail_rows(&one, 1, Some(0), 0, 50)),
            [(true, true, true, true)],
            "one entry is both ends, filled and current"
        );
        assert_eq!(marks(&rail_rows(&one, 0, None, 0, 50)), [(false, false, true, true)]);
        let nested = list(&[0, 1, 1, 2, 0]);
        assert_eq!(
            marks(&rail_rows(&nested, 3, Some(2), 0, 50)),
            [
                (false, true, true, false),
                (false, true, false, false),
                (true, true, false, false),
                (false, false, false, false),
                (false, false, false, true)
            ],
            "filled down to the entries counted, current on one"
        );
        assert_eq!(
            marks(&rail_rows(&nested, 5, None, 3, 50)),
            [(false, true, false, false), (false, true, false, true)],
            "a window taken from the middle keeps the list's ends"
        );
        // Entries on one page, and one claiming a page past the last: the rail counts entries and says nothing about pages.
        let mut same = list(&[0, 0, 0]);
        for entry in &mut same {
            entry.page = Some(4);
        }
        same[2].page = Some(u32::MAX);
        assert_eq!(
            marks(&rail_rows(&same, 2, Some(1), 0, 50)),
            [(false, true, true, false), (true, true, false, false), (false, false, false, true)]
        );
        assert_eq!(rail_rows(&nested, 5, None, 5, 50), vec![], "a window past the end is empty");
        assert_eq!(rail_rows(&nested, 5, None, u32::MAX, u32::MAX), vec![], "and so is one at the largest numbers");
        assert_eq!(rail_rows(&nested, 9, Some(7), 4, 1).len(), 1);
        assert!(
            rail_rows(&nested, 9, Some(7), 4, 1)[0].rail_filled && !rail_rows(&nested, 9, Some(7), 4, 1)[0].current,
            "a fill or a current beyond the list marks no row wrongly"
        );
    }

    /// **Failure: a hostile outline at the cap makes the fetch grow with what the file claims or the host asks:** more than [`crate::core::MAX_CONTENTS_FETCH`] rows
    /// in one call, or rows from the wrong place in a list of the most entries a file may carry (the cap is tested at the limit and one past it).
    #[test]
    fn a_fetch_from_a_list_at_the_cap_hands_over_no_more_than_the_cap() {
        let cap = crate::core::MAX_CONTENTS_FETCH as usize;
        let big = list(&vec![0; cap * 4 + 1]);
        assert_eq!(rail_rows(&big, u32::MAX, None, 0, u32::MAX).len(), cap, "asking for everything gets the cap");
        assert_eq!(rail_rows(&big, 0, None, 0, cap as u32 + 1).len(), cap, "one past the cap");
        assert_eq!(rail_rows(&big, 0, None, 0, cap as u32).len(), cap, "at the cap");
        let tail = rail_rows(&big, 0, None, (cap * 4) as u32 - 1, 100);
        assert_eq!(tail.len(), 2, "the tail of the list");
        assert!(tail[1].rail_last && !tail[0].rail_last && tail[1].text == "Entry 1024");
    }
}
