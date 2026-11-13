//! **The sidebar's contents, as the person uses them** (step R2.3, C5f): a section's chevron is `contents fold`, and the note over a tab's contents is
//! `contents show own|generated` or `contents stop`, the commands an agent sends, run for the person. Which entry a chevron is on, and what the note offers,
//! are what the session published.

use super::{Inner, lock, person};
use recto_agent::commands::contents::{ContentsFold, ContentsShow, ContentsStop, Which};
use recto_agent::{Command, TabRef};
use recto_session::{ContentsAction, TabId};
use std::sync::Arc;

/// What the note's action comes to: Generate and Show generated are both `contents show generated`, which makes them if they aren't made.
fn note_command(tab: TabId, action: ContentsAction) -> Command {
    let tab = TabRef::Id(tab);
    let show = |which: Which| ContentsShow { which, tab: tab.clone() }.into();
    match action {
        ContentsAction::Generate | ContentsAction::ShowGenerated => show(Which::Generated),
        ContentsAction::ShowOwn => show(Which::Own),
        ContentsAction::Stop => ContentsStop { tab: tab.clone() }.into(),
    }
}

impl Inner {
    /// A tap on the note over a tab's contents.
    pub fn contents_note_for_person(self: &Arc<Self>, tab: TabId, action: ContentsAction) {
        person::submit(self, note_command(tab, action));
    }

    /// A contents row's chevron: `contents fold`, its section folded or opened as it is when it runs. A row with nothing under it has no chevron, and
    /// does nothing.
    pub fn toggle_contents_section(self: &Arc<Self>, first: u32) {
        let entry = lock(&self.published)
            .contents
            .get(first as usize)
            .map(|entry| (entry.tab, entry.entry, entry.expandable));
        if let Some((tab, entry, true)) = entry {
            person::submit(
                self,
                ContentsFold {
                    entry,
                    tab: TabRef::Id(TabId(tab)),
                }
                .into(),
            );
        }
    }
}
