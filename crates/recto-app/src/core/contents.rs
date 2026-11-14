//! **The sidebar's contents, as the person uses them** (step R2.3, C5f): a section's chevron is `contents fold`, and the note over a tab's contents is
//! `contents show own|generated` or `contents stop`, the commands an agent sends, run for the person. Which entry a chevron is on, and what the note offers,
//! are what the session published.

use recto_agent::commands::contents::{ContentsFold, ContentsShow, ContentsStop, Which};
use recto_agent::{Command, TabRef};
use recto_session::{ContentsAction, TabId};

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
        Some(entry) if !entry.expandable => Some(format!("Row {row} has no sections under it, so it has nothing to fold or open.")),
        Some(_) => None,
    }
}
