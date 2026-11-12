//! What a save needs of the app beyond the session: where the backup of an original goes, and its journals. A save's note goes to the journal and is
//! read back from the disk before the file's moved into place, so a crash just after the move finds it and carries the history over.

use super::saver::Saver;
pub use crate::saving::SaveProblem;
use crate::saving::{self as app, replace_answered};
pub use crate::saving::{CloseButton, CloseChoice, ClosePrompt, SavePanelModel};
pub use crate::saving::{SAVED_NOT_CLOSED, SaveUi, close_prompt, panel_for, save_from_gate};
use recto_agent::saving::SaveSupport;
use recto_core::save::SaveStep;
use recto_platform::FileStamp;
use recto_session::TabId;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

pub struct AppSaving {
    pub saver: Arc<Saver>,
    pub backups: PathBuf,
    /// How the save under way is going, for the window's progress: 0 for none.
    pub step: Arc<AtomicU8>,
    pub core: Arc<super::CoreQueue>,
}

impl SaveSupport for AppSaving {
    fn backups(&self) -> Option<PathBuf> {
        Some(self.backups.clone())
    }

    fn before_writing(&self, _tab: TabId) {
        if let Some(inner) = self.saver.inner() {
            self.saver.write_journals_now(&inner);
        }
    }

    fn note_on_disk(&self, tab: TabId, target: FileStamp) -> Result<(), String> {
        let inner = self.saver.inner().ok_or("the app is going away")?;
        self.saver.write_journals_now(&inner);
        let name = inner.host.session.with_session(|session| session.journal_of(tab).map(String::from));
        match name {
            // A session that keeps no journal has no crash to carry a save across.
            None => Ok(()),
            Some(name) if self.saver.store.journal_says_saving(&name, target) => Ok(()),
            Some(_) => Err("the save's note didn't reach the journal".into()),
        }
    }

    fn keep_pages(&self, _tab: TabId, original: &std::path::Path, target: FileStamp, pages: &[u32]) -> Option<recto_session::edit::AddedSource> {
        self.saver.store.keep_pages_of(original, target, pages)
    }

    fn after_save(&self, _tab: TabId) {
        self.step.store(0, Ordering::SeqCst);
        // A Save As moved the tab to another file, and a save renamed its journal: the record follows straight away, not in a minute.
        self.saver.write_soon();
    }

    fn step(&self, _tab: TabId, step: SaveStep) {
        let value = match step {
            SaveStep::Checking => 1,
            SaveStep::Writing => 2,
            SaveStep::Prepared => 3,
            SaveStep::Committing => 4,
        };
        self.step.store(value, Ordering::SeqCst);
        // So the window shows it's saving, and which step a long save is at.
        self.core.push(super::CoreCall::Reconcile);
    }
}

/// **A save from the window, off the UI thread,** through the same command an agent's `recto save` runs, at the owner's level. `out` makes it a Save As. `close_after` closes the tab once it's saved, for the close prompt's Save. A hand-off waits for it, as it's a running command.
pub fn start_save(inner: &Arc<super::Inner>, tab: TabId, out: Option<std::path::PathBuf>, close_after: bool) {
    // When the person answered Save As's sheet: a file made at that name after this isn't one they agreed to replace (review).
    let answered = std::time::SystemTime::now();
    let inner = Arc::clone(inner);
    inner.tools.save_ui.running.fetch_add(1, Ordering::SeqCst);
    let spawned = std::thread::Builder::new().name("recto-save-file".into()).stack_size(1024 * 1024).spawn({
        let inner = Arc::clone(&inner);
        move || {
            run_save(&inner, tab, out, close_after, answered);
            inner.tools.save_ui.running.fetch_sub(1, Ordering::SeqCst);
            inner.tools.save_ui.step.store(0, Ordering::SeqCst);
            inner.core.push(super::CoreCall::Reconcile);
        }
    });
    if spawned.is_err() {
        inner.tools.save_ui.running.fetch_sub(1, Ordering::SeqCst);
        inner.tools.save_ui.set_problem(Some(app::not_started()));
        inner.core.push(super::CoreCall::Reconcile);
    }
}

/// The save of the tab the action was for, whichever is showing by now. A Save As's sheet has already asked whether to replace a file that's there, so one
/// that's there was agreed to; it's looked at here, off the UI thread (review).
fn run_save(inner: &super::Inner, id: TabId, out: Option<std::path::PathBuf>, close_after: bool, answered: std::time::SystemTime) {
    use recto_control::host::Host;
    let replace = match out.as_deref().map(|out| replace_answered(out, answered)) {
        Some(Err(())) => {
            inner.tools.save_ui.set_problem(Some(app::appeared_after_answer()));
            return;
        }
        Some(Ok(replace)) => replace,
        None => false,
    };
    let command = recto_agent::Command::Save {
        tab: format!("#{}", id.0),
        out,
        replace,
    };
    let mut events = Vec::new();
    match inner.host.run(recto_agent::Level::Full, &command, &mut events) {
        Ok(data) => {
            let notes: Vec<&str> = data["notes"]
                .as_array()
                .map(|notes| notes.iter().filter_map(|note| note.as_str()).collect())
                .unwrap_or_default();
            *super::lock(&inner.window.action_notice) = (!notes.is_empty()).then(|| notes.join(" "));
            inner.tools.save_ui.set_problem(None);
            // Watchers hear it was saved before they hear it closed.
            inner.core.push(super::CoreCall::Events(std::mem::take(&mut events)));
            if close_after {
                inner.core.push(super::CoreCall::CloseAfterSave(id));
            }
        }
        Err(error) => inner.tools.save_ui.set_problem(Some(app::problem_for(&error))),
    }
    inner.core.push(super::CoreCall::Events(events));
}

/// **Saving and closing, as the window asks:** Save As's sheet and its answer, what closing a document asks and its answer, and a save's problem answered. What they
/// say is `crate::saving`'s; the save itself, [`start_save`]'s.
impl super::Inner {
    /// What Save As's sheet says, for the tab showing. Never waits: from what's published.
    pub fn save_panel(&self) -> SavePanelModel {
        let reader = super::lock(&self.published).reader.clone();
        panel_for(reader.shown_tab, &reader.document_name, &reader.document_folder, reader.save_as_note)
    }

    /// Save As's sheet was answered with this path.
    pub fn save_as(self: &Arc<Self>, tab: u32, path: String) {
        if self.core.frozen.load(Ordering::SeqCst) || tab == 0 {
            return;
        }
        start_save(self, TabId(tab), Some(PathBuf::from(path)), false);
    }

    /// What closing the document showing asks.
    pub fn close_prompt(&self) -> ClosePrompt {
        let published = super::lock(&self.published);
        // Save offered only where Save works: with it off for a signed document, the way on is Save As's unsigned copy.
        let save_off = published.reader.edited && !published.reader.save_enabled && !published.reader.saving;
        close_prompt(
            published.reader.shown_tab,
            &published.reader.document_name,
            published.reader.edited,
            published.sidebar.undone_owner_steps,
            save_off,
            self.tools.save_ui.disagreement_now().as_deref(),
        )
    }

    /// The close prompt's answer. Save closes once it's saved, and not if the save fails. Don't Save throws the edits away and closes. Save As opens its sheet, and the tab stays.
    pub fn close_answered(&self, tab: u32, choice: CloseChoice) -> super::HostAction {
        if tab == 0 {
            return super::HostAction::None;
        }
        if choice == CloseChoice::SaveAs {
            return super::HostAction::ShowSavePanel;
        }
        self.core.push(super::CoreCall::CloseAnswered(TabId(tab), choice));
        super::HostAction::None
    }

    /// A save's problem was answered: with Save As, its sheet opens.
    pub fn save_problem_answered(&self, save_as: bool) -> super::HostAction {
        self.tools.save_ui.set_problem(None);
        self.core.push(super::CoreCall::Reconcile);
        if save_as { super::HostAction::ShowSavePanel } else { super::HostAction::None }
    }
}
