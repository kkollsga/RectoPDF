//! What a save needs of the app beyond the session: where the backup of an original goes, and its journals. A save's note goes to the journal and is
//! read back from the disk before the file's moved into place, so a crash just after the move finds it and carries the history over.

use crate::saver::Saver;
use recto_agent::saving::SaveSupport;
pub use recto_app::saving::SaveProblem;
use recto_app::saving::{self as app, replace_answered};
pub(crate) use recto_app::saving::{SAVED_NOT_CLOSED, SaveUi, close_prompt, panel_for, save_from_gate};
use recto_core::save::SaveStep;
use recto_platform::FileStamp;
use recto_session::TabId;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

pub(crate) struct AppSaving {
    pub(crate) saver: Arc<Saver>,
    pub(crate) backups: PathBuf,
    /// How the save under way is going, for the window's progress: 0 for none.
    pub(crate) step: Arc<AtomicU8>,
    pub(crate) core: Arc<crate::CoreQueue>,
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
        self.core.push(crate::CoreCall::Reconcile);
    }
}

/// A save that didn't happen, as an alert over the window. Every word is Rust's.
#[uniffi::remote(Record)]
pub struct SaveProblem {
    pub title: String,
    pub message: String,
    /// A button that opens Save As, when a copy is the way on.
    pub save_as_title: Option<String>,
    pub ok_title: String,
}

/// What Save As's sheet says.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SavePanelModel {
    /// The tab it's for, as it was when the sheet opened: the answer saves that tab, whichever is showing by then.
    pub tab: u32,
    pub prompt: String,
    pub suggested_name: String,
    /// The folder it opens in: the document's own.
    pub directory: String,
    /// A line on the sheet, as for a signed document. `None` for most.
    pub message: Option<String>,
}

/// What closing a document with unsaved edits asks, as any Mac app asks it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ClosePrompt {
    /// The tab it's about, as it was when it was asked.
    pub tab: u32,
    pub message: String,
    pub informative: String,
    /// In the order they're shown, each with what it does.
    pub buttons: Vec<CloseButton>,
}

/// One of the close prompt's buttons.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CloseButton {
    pub title: String,
    pub choice: CloseChoice,
    /// Throws something away, so it's drawn as such.
    pub destructive: bool,
}

/// The answer to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum CloseChoice {
    Save,
    /// Save is off for this document, as for a signed one: Save As's sheet, for an unsigned copy.
    SaveAs,
    DontSave,
    Cancel,
}

impl From<app::SavePanelModel> for SavePanelModel {
    fn from(panel: app::SavePanelModel) -> Self {
        Self {
            tab: panel.tab,
            prompt: panel.prompt,
            suggested_name: panel.suggested_name,
            directory: panel.directory,
            message: panel.message,
        }
    }
}

impl From<app::ClosePrompt> for ClosePrompt {
    fn from(prompt: app::ClosePrompt) -> Self {
        Self {
            tab: prompt.tab,
            message: prompt.message,
            informative: prompt.informative,
            buttons: prompt.buttons.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<app::CloseButton> for CloseButton {
    fn from(button: app::CloseButton) -> Self {
        Self {
            title: button.title,
            choice: button.choice.into(),
            destructive: button.destructive,
        }
    }
}

impl From<app::CloseChoice> for CloseChoice {
    fn from(choice: app::CloseChoice) -> Self {
        match choice {
            app::CloseChoice::Save => Self::Save,
            app::CloseChoice::SaveAs => Self::SaveAs,
            app::CloseChoice::DontSave => Self::DontSave,
            app::CloseChoice::Cancel => Self::Cancel,
        }
    }
}

impl From<CloseChoice> for app::CloseChoice {
    fn from(choice: CloseChoice) -> Self {
        match choice {
            CloseChoice::Save => Self::Save,
            CloseChoice::SaveAs => Self::SaveAs,
            CloseChoice::DontSave => Self::DontSave,
            CloseChoice::Cancel => Self::Cancel,
        }
    }
}

/// **A save from the window, off the UI thread,** through the same command an agent's `recto save` runs, at the owner's level. `out` makes it a Save As. `close_after` closes the tab once it's saved, for the close prompt's Save. A hand-off waits for it, as it's a running command.
pub(crate) fn start_save(inner: &Arc<crate::Inner>, tab: TabId, out: Option<std::path::PathBuf>, close_after: bool) {
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
            inner.core.push(crate::CoreCall::Reconcile);
        }
    });
    if spawned.is_err() {
        inner.tools.save_ui.running.fetch_sub(1, Ordering::SeqCst);
        inner.tools.save_ui.set_problem(Some(app::not_started()));
        inner.core.push(crate::CoreCall::Reconcile);
    }
}

/// The save of the tab the action was for, whichever is showing by now. A Save As's sheet has already asked whether to replace a file that's there, so one
/// that's there was agreed to; it's looked at here, off the UI thread (review).
fn run_save(inner: &crate::Inner, id: TabId, out: Option<std::path::PathBuf>, close_after: bool, answered: std::time::SystemTime) {
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
            *crate::lock(&inner.window.action_notice) = (!notes.is_empty()).then(|| notes.join(" "));
            inner.tools.save_ui.set_problem(None);
            // Watchers hear it was saved before they hear it closed.
            inner.core.push(crate::CoreCall::Events(std::mem::take(&mut events)));
            if close_after {
                inner.core.push(crate::CoreCall::CloseAfterSave(id));
            }
        }
        Err(error) => inner.tools.save_ui.set_problem(Some(app::problem_for(&error))),
    }
    inner.core.push(crate::CoreCall::Events(events));
}
