//! What a save needs of the app beyond the session: where the backup of an original goes, and its journals. A save's note goes to the journal and is
//! read back from the disk before the file's moved into place, so a crash just after the move finds it and carries the history over.

use crate::saver::Saver;
use recto_agent::saving::SaveSupport;
use recto_core::save::SaveStep;
use recto_platform::FileStamp;
use recto_session::TabId;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

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
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
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
    pub message: String,
    pub informative: String,
    pub save_title: String,
    pub dont_save_title: String,
    pub cancel_title: String,
}

/// The answer to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum CloseChoice {
    Save,
    DontSave,
    Cancel,
}

/// The decided words for signed copies (SPEC decision 27).
pub(crate) const UNSIGNED_COPY: &str = "The new copy won't be signed. The original keeps its signature.";
/// Whether Save As makes that copy yet. It's the streaming rewrite's, the next stage of Save.
const UNSIGNED_COPY_BUILT: bool = false;

/// The app's side of saving that the window draws: how many saves are running, a problem waiting to be answered, and what's known of the tab showing's signatures.
pub(crate) struct SaveUi {
    pub(crate) running: AtomicU32,
    problem: Mutex<Option<SaveProblem>>,
    /// The tab showing's file, as of this version, and what Save As would say of it. Looked at once a version, on the session thread.
    signature: Mutex<Option<(TabId, FileStamp, Option<String>)>>,
    pub(crate) step: Arc<AtomicU8>,
}

impl SaveUi {
    pub(crate) fn new(step: Arc<AtomicU8>) -> Self {
        Self { running: AtomicU32::new(0), problem: Mutex::new(None), signature: Mutex::new(None), step }
    }

    /// What Save As's sheet says of this file: that a certified document's copy with its pages changed won't be signed. Read from the file's catalog once a version, never on the UI thread.
    pub(crate) fn signature_note(&self, id: TabId, path: &std::path::Path, stamp: FileStamp) -> Option<String> {
        let mut known = crate::lock(&self.signature);
        if let Some((tab, at, note)) = known.as_ref()
            && *tab == id
            && *at == stamp
        {
            return note.clone();
        }
        // Said on the sheet once Save As can make that copy, which is the rewrite's: until then saying it would promise what isn't there.
        let note = recto_core::save::signature_facts_of(path).ok().filter(|facts| facts.certified && UNSIGNED_COPY_BUILT).map(|_| UNSIGNED_COPY.to_string());
        *known = Some((id, stamp, note.clone()));
        note
    }

    /// What the Save icon says: Save, or the step a long save is at.
    pub(crate) fn tooltip(&self) -> String {
        match self.step.load(Ordering::SeqCst) {
            1 => "Saving: checking every page against the file…",
            2 => "Saving: writing the file…",
            3 | 4 => "Saving: checking what was written, and putting it in place…",
            _ => "Save (⌘S)",
        }
        .to_string()
    }

    pub(crate) fn problem(&self) -> Option<SaveProblem> {
        crate::lock(&self.problem).clone()
    }

    pub(crate) fn set_problem(&self, problem: Option<SaveProblem>) {
        *crate::lock(&self.problem) = problem;
    }
}

/// The problem a failed save shows: what went wrong, in the save's own words, and Save As where a copy is the way on.
pub(crate) fn problem_for(error: &recto_agent::AgentError) -> SaveProblem {
    // A certified document's unsigned copy is the rewrite's, which comes next: until then Save As can't help it.
    let copy_helps = matches!(error.code, "file_changed" | "read_only" | "hard_links");
    SaveProblem { title: "The document couldn't be saved.".into(), message: error.message.clone(), save_as_title: copy_helps.then(|| "Save As…".to_string()), ok_title: "OK".into() }
}

/// Save As's sheet for the tab showing.
pub(crate) fn panel_for(path: Option<&std::path::Path>, note: Option<String>) -> SavePanelModel {
    let suggested = path.and_then(|path| path.file_stem()).map(|stem| format!("{} copy.pdf", recto_core::clean_line(&stem.to_string_lossy()))).unwrap_or_else(|| "Untitled.pdf".into());
    let directory = path.and_then(std::path::Path::parent).map(|folder| folder.display().to_string()).unwrap_or_default();
    SavePanelModel { prompt: "Save".into(), suggested_name: suggested, directory, message: note }
}

/// What closing asks, about this document.
pub(crate) fn close_prompt(name: &str) -> ClosePrompt {
    ClosePrompt { message: format!("Do you want to save the changes you made to “{}”?", recto_core::clean_line(name)), informative: "Your changes will be lost if you don't save them.".into(), save_title: "Save".into(), dont_save_title: "Don't Save".into(), cancel_title: "Cancel".into() }
}

/// **A save from the window, off the UI thread,** through the same command an agent's `recto save` runs, at the owner's level. `out` makes it a Save As. `close_after` closes the tab once it's saved, for the close prompt's Save. A hand-off waits for it, as it's a running command.
pub(crate) fn start_save(inner: &Arc<crate::Inner>, out: Option<(std::path::PathBuf, bool)>, close_after: bool) {
    let inner = Arc::clone(inner);
    inner.save_ui.running.fetch_add(1, Ordering::SeqCst);
    let spawned = std::thread::Builder::new().name("recto-save-file".into()).stack_size(1024 * 1024).spawn({
        let inner = Arc::clone(&inner);
        move || {
            run_save(&inner, out, close_after);
            inner.save_ui.running.fetch_sub(1, Ordering::SeqCst);
            inner.save_ui.step.store(0, Ordering::SeqCst);
            inner.core.push(crate::CoreCall::Reconcile);
        }
    });
    if spawned.is_err() {
        inner.save_ui.running.fetch_sub(1, Ordering::SeqCst);
        inner.save_ui.set_problem(Some(SaveProblem { title: "The document couldn't be saved.".into(), message: "The save couldn't be started, so nothing was written.".into(), save_as_title: None, ok_title: "OK".into() }));
        inner.core.push(crate::CoreCall::Reconcile);
    }
}

fn run_save(inner: &crate::Inner, out: Option<(std::path::PathBuf, bool)>, close_after: bool) {
    use recto_control::host::Host;
    let Some(id) = inner.host.session.with_session(|session| session.active()) else { return };
    let (out, replace) = match out {
        Some((out, replace)) => (Some(out), replace),
        None => (None, false),
    };
    let command = recto_agent::Command::Save { tab: format!("#{}", id.0), out, replace };
    let mut events = Vec::new();
    match inner.host.run(recto_agent::Level::Full, &command, &mut events) {
        Ok(data) => {
            let notes: Vec<&str> = data["notes"].as_array().map(|notes| notes.iter().filter_map(|note| note.as_str()).collect()).unwrap_or_default();
            *crate::lock(&inner.action_notice) = (!notes.is_empty()).then(|| notes.join(" "));
            inner.save_ui.set_problem(None);
            if close_after {
                inner.core.push(crate::CoreCall::CloseTab(id, false));
            }
        }
        Err(error) => inner.save_ui.set_problem(Some(problem_for(&error))),
    }
    inner.core.push(crate::CoreCall::Events(events));
}
