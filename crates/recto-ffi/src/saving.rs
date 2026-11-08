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
    DontSave,
    Cancel,
}

/// What the window says when the prompt's Save saved but an edit since kept the tab open.
pub(crate) const SAVED_NOT_CLOSED: &str = "It was saved, but not closed: there are new changes since.";

/// **What Save As's answer at `answered` agreed to:** replacing a file that was at that name then (the sheet asked), `Ok(true)`; a new file, `Ok(false)`; and a file made there after the answer, which nobody agreed to replace, `Err(())` (review round 2).
pub(crate) fn replace_answered(out: &std::path::Path, answered: std::time::SystemTime) -> Result<bool, ()> {
    match std::fs::symlink_metadata(out) {
        Err(_) => Ok(false),
        Ok(there) if there.created().is_ok_and(|made| made > answered) => Err(()),
        Ok(_) => Ok(true),
    }
}

/// The decided words for signed copies (SPEC decision 27).
pub(crate) const UNSIGNED_COPY: &str = recto_agent::saving::UNSIGNED_COPY;

/// The app's side of saving that the window draws: how many saves are running, a problem waiting to be answered, and what's known of the tab showing's signatures.
pub(crate) struct SaveUi {
    pub(crate) running: AtomicU32,
    problem: Mutex<Option<SaveProblem>>,
    /// The tab showing's file, as of this version, and what's known of its signatures. Looked at once a version, on the session thread.
    signature: Mutex<Option<(TabId, FileStamp, recto_core::save::SignatureFacts)>>,
    pub(crate) step: Arc<AtomicU8>,
}

impl SaveUi {
    pub(crate) fn new(step: Arc<AtomicU8>) -> Self {
        Self { running: AtomicU32::new(0), problem: Mutex::new(None), signature: Mutex::new(None), step }
    }

    /// What's known of this file's signatures: read from its catalog and form once a version, never on the UI thread. A file that won't open says nothing.
    pub(crate) fn signature_facts(&self, id: TabId, path: &std::path::Path, stamp: FileStamp) -> recto_core::save::SignatureFacts {
        let mut known = crate::lock(&self.signature);
        if let Some((tab, at, facts)) = known.as_ref()
            && *tab == id
            && *at == stamp
        {
            return *facts;
        }
        let facts = recto_core::save::signature_facts_of(path).unwrap_or_default();
        *known = Some((id, stamp, facts));
        facts
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
    // A signed document's copy is unsigned (SPEC decision 27), and the original keeps its signature: Save As is the way on for those too.
    let copy_helps = matches!(error.code, "file_changed" | "read_only" | "hard_links" | "breaks_certification" | "locked_by_signature" | "signed_needs_copy");
    SaveProblem { title: "The document couldn't be saved.".into(), message: error.message.clone(), save_as_title: copy_helps.then(|| "Save As…".to_string()), ok_title: "OK".into() }
}

/// Save As's sheet for the tab showing.
pub(crate) fn panel_for(tab: u32, name: &str, folder: &str, note: Option<String>) -> SavePanelModel {
    let stem = std::path::Path::new(name).file_stem().map(|stem| recto_core::clean_line(&stem.to_string_lossy())).filter(|stem| !stem.is_empty());
    let suggested = stem.map_or_else(|| "Untitled.pdf".into(), |stem| format!("{stem} copy.pdf"));
    SavePanelModel { tab, prompt: "Save".into(), suggested_name: suggested, directory: folder.to_string(), message: note }
}

/// What closing asks, about this document.
/// **What closing asks, decided here** (review): with unsaved edits, Save, Cancel or Don't Save, as any Mac app asks. Steps the person undid can't be redone
/// once the tab's closed, so it says so, and closing after either answer lets them go. With only undone steps, there's nothing to save: Close or Cancel.
pub(crate) fn close_prompt(tab: u32, name: &str, edited: bool, undone: bool) -> ClosePrompt {
    let name = recto_core::clean_line(name);
    let button = |title: &str, choice, destructive| CloseButton { title: title.into(), choice, destructive };
    if edited {
        let informative = if undone { "Your changes will be lost if you don't save them. Steps you undid can't be redone once it's closed." } else { "Your changes will be lost if you don't save them." };
        ClosePrompt { tab, message: format!("Do you want to save the changes you made to “{name}”?"), informative: informative.into(), buttons: vec![button("Save", CloseChoice::Save, false), button("Cancel", CloseChoice::Cancel, false), button("Don't Save", CloseChoice::DontSave, true)] }
    } else {
        ClosePrompt { tab, message: format!("Close “{name}”?"), informative: "It has no unsaved changes, but steps you undid can't be redone once it's closed.".into(), buttons: vec![button("Close", CloseChoice::DontSave, true), button("Cancel", CloseChoice::Cancel, false)] }
    }
}

/// **A save from the window, off the UI thread,** through the same command an agent's `recto save` runs, at the owner's level. `out` makes it a Save As. `close_after` closes the tab once it's saved, for the close prompt's Save. A hand-off waits for it, as it's a running command.
pub(crate) fn start_save(inner: &Arc<crate::Inner>, tab: TabId, out: Option<std::path::PathBuf>, close_after: bool) {
    // When the person answered Save As's sheet: a file made at that name after this isn't one they agreed to replace (review).
    let answered = std::time::SystemTime::now();
    let inner = Arc::clone(inner);
    inner.save_ui.running.fetch_add(1, Ordering::SeqCst);
    let spawned = std::thread::Builder::new().name("recto-save-file".into()).stack_size(1024 * 1024).spawn({
        let inner = Arc::clone(&inner);
        move || {
            run_save(&inner, tab, out, close_after, answered);
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

/// The save of the tab the action was for, whichever is showing by now. A Save As's sheet has already asked whether to replace a file that's there, so one
/// that's there was agreed to; it's looked at here, off the UI thread (review).
fn run_save(inner: &crate::Inner, id: TabId, out: Option<std::path::PathBuf>, close_after: bool, answered: std::time::SystemTime) {
    use recto_control::host::Host;
    let replace = match out.as_deref().map(|out| replace_answered(out, answered)) {
        Some(Err(())) => {
            inner.save_ui.set_problem(Some(SaveProblem { title: "The document couldn't be saved.".into(), message: "A file appeared at that name after you chose it, so it wasn't replaced. Nothing was written.".into(), save_as_title: Some("Save As…".into()), ok_title: "OK".into() }));
            return;
        }
        Some(Ok(replace)) => replace,
        None => false,
    };
    let command = recto_agent::Command::Save { tab: format!("#{}", id.0), out, replace };
    let mut events = Vec::new();
    match inner.host.run(recto_agent::Level::Full, &command, &mut events) {
        Ok(data) => {
            let notes: Vec<&str> = data["notes"].as_array().map(|notes| notes.iter().filter_map(|note| note.as_str()).collect()).unwrap_or_default();
            *crate::lock(&inner.action_notice) = (!notes.is_empty()).then(|| notes.join(" "));
            inner.save_ui.set_problem(None);
            // Watchers hear it was saved before they hear it closed.
            inner.core.push(crate::CoreCall::Events(std::mem::take(&mut events)));
            if close_after {
                inner.core.push(crate::CoreCall::CloseAfterSave(id));
            }
        }
        Err(error) => inner.save_ui.set_problem(Some(problem_for(&error))),
    }
    inner.core.push(crate::CoreCall::Events(events));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure (review round 2): Save As decides "replace" from the disk at save time, so a file someone makes at the chosen name after the person answered "new" is replaced, though nobody agreed to that. A file there at the answer is replaced, as the sheet asked; one made after is refused; none is a new file.
    #[test]
    fn save_as_keeps_the_answer_the_person_gave() {
        let dir = std::env::temp_dir().join(format!("recto-answer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let before = dir.join("there-before.pdf");
        std::fs::write(&before, b"x").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let answered = std::time::SystemTime::now();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let after = dir.join("made-after.pdf");
        std::fs::write(&after, b"y").unwrap();
        assert_eq!(replace_answered(&before, answered), Ok(true), "there at the answer: the sheet asked");
        assert_eq!(replace_answered(&after, answered), Err(()), "made after the answer: refused");
        assert_eq!(replace_answered(&dir.join("none.pdf"), answered), Ok(false), "nothing there: a new file");
        let _ = std::fs::remove_dir_all(dir);
    }
}
