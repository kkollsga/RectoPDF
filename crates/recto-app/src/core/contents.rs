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
