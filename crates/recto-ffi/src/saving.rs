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
    /// Save is off for this document, as for a signed one: Save As's sheet, for an unsigned copy.
    SaveAs,
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

/// What the Save icon says when it's off for a signed document.
const SAVE_OFF_TOOLTIP: &str = "This document is signed, and saving this change would take the signature off, so Save is off. Save As makes an unsigned copy.";

/// **Whether a Save over this file would be refused for its signature,** so Save is off and Save As's copy is unsigned (SPEC decision 27): a person's signature and a change that has to be written anew, or a certification or lock and any change. Usage rights alone don't. The same rule `recto_core::save::save` refuses by.
pub(crate) fn save_needs_copy(facts: recto_core::save::SignatureFacts, needs_rewrite: bool) -> bool {
    facts.has_persons_signature() && (needs_rewrite || facts.certified || facts.field_locked)
}

/// What the window's Save shows for the tab showing, decided here: whether it's off, what Save As's sheet says, and the icon's tooltip.
pub(crate) struct SaveState {
    pub(crate) off: bool,
    pub(crate) note: Option<String>,
    pub(crate) tooltip: String,
}

/// **The agreement gate for `qa save-from`,** which is refused where Save would be: a file whose readers disagree, or that couldn't be checked.
pub(crate) fn save_from_gate(path: &std::path::Path, known: Option<FileStamp>) -> Result<(), recto_agent::AgentError> {
    match known.and_then(|stamp| recto_agent::agreement::check(path, stamp)) {
        Some(disagreement) => Err(recto_agent::AgentError::new(
            "parsers_disagree",
            disagreement.message(),
            recto_agent::exit::FAILED,
        )),
        None => Ok(()),
    }
}

/// The app's side of saving that the window draws: how many saves are running, a problem waiting to be answered, and what's known of the tab showing's signatures.
pub(crate) struct SaveUi {
    pub(crate) running: AtomicU32,
    problem: Mutex<Option<SaveProblem>>,
    /// What's known of the signatures of the last few files asked about, each as of a version, newest last. Looked at once a version, on a thread of its own, as it
    /// reads the whole file. At most [`SIGNATURE_LOOKS`] are kept.
    signature: Arc<Mutex<Vec<(TabId, FileStamp, recto_core::save::SignatureFacts)>>>,
    /// The looks under way, one per file and version, at most [`SIGNATURE_LOOKS`]: one slow disk doesn't hold up every other tab's (review round 2).
    looking: Arc<Mutex<Vec<(TabId, FileStamp)>>>,
    pub(crate) step: Arc<AtomicU8>,
    /// Why Save and Save As are off for the tab showing, when its two readers disagree about its file (the agreement gate), for the close prompt.
    disagreement: Mutex<Option<String>>,
}

/// How many signature looks run at once, and how many answers are kept.
pub(crate) const SIGNATURE_LOOKS: usize = 4;

/// **A look under way ends however its thread does** (review round 2): it's taken off the list when the thread ends, and a look that panicked counts as
/// having found nothing, as a file that won't open does, so it isn't started again at every update of the window. The save itself looks again, and refuses
/// what it can't look through.
struct LookEnds {
    key: (TabId, FileStamp),
    known: Arc<Mutex<Vec<(TabId, FileStamp, recto_core::save::SignatureFacts)>>>,
    looking: Arc<Mutex<Vec<(TabId, FileStamp)>>>,
}

impl Drop for LookEnds {
    fn drop(&mut self) {
        if std::thread::panicking() {
            remember(&self.known, self.key, recto_core::save::SignatureFacts::default());
        }
        crate::lock(&self.looking).retain(|key| *key != self.key);
    }
}

/// Keeps `facts` as the answer for `key`, in place of any older answer for that tab, dropping the oldest past [`SIGNATURE_LOOKS`].
fn remember(known: &Mutex<Vec<(TabId, FileStamp, recto_core::save::SignatureFacts)>>, key: (TabId, FileStamp), facts: recto_core::save::SignatureFacts) {
    let mut known = crate::lock(known);
    known.retain(|(tab, _, _)| *tab != key.0);
    known.push((key.0, key.1, facts));
    if known.len() > SIGNATURE_LOOKS {
        known.remove(0);
    }
}

impl SaveUi {
    pub(crate) fn new(step: Arc<AtomicU8>) -> Self {
        Self {
            running: AtomicU32::new(0),
            problem: Mutex::new(None),
            signature: Arc::new(Mutex::new(Vec::new())),
            looking: Arc::new(Mutex::new(Vec::new())),
            step,
            disagreement: Mutex::new(None),
        }
    }

    /// **The agreement gate, as the window sees it** (SPEC section 3): why Save and Save As are off for this file's version, in Rust's words, or
    /// `None` when its readers agree or it isn't known yet. Not known starts the look off this thread (`recto_agent::agreement`), and `done` is
    /// called when it's known. The save asks again itself, so a file not known yet is still never saved from a disagreeing reading.
    pub(crate) fn disagreement(&self, file: Option<(&std::path::Path, FileStamp)>, done: impl FnOnce() + Send + 'static) -> Option<String> {
        let why = file
            .and_then(|(path, stamp)| recto_agent::agreement::known_or_start(path, stamp, done))
            .flatten()
            .map(|disagreement| disagreement.message());
        crate::lock(&self.disagreement).clone_from(&why);
        why
    }

    /// **What the window's Save is, for the tab showing:** off when a Save over the file would break a signature (Save As then makes an unsigned
    /// copy, and the sheet says so) or when its two readers disagree (Save As is off too, and the tooltip says why, which comes first). `file` is
    /// the tab's file with edits to save, or `None`.
    pub(crate) fn save_state(
        &self,
        facts: recto_core::save::SignatureFacts,
        needs_rewrite: bool,
        file: Option<(&std::path::Path, FileStamp)>,
        done: impl FnOnce() + Send + 'static,
    ) -> SaveState {
        let disagreement = self.disagreement(file, done);
        let signed_off = save_needs_copy(facts, needs_rewrite);
        let tooltip = match (&disagreement, signed_off) {
            (Some(why), _) => why.clone(),
            (None, true) => SAVE_OFF_TOOLTIP.to_string(),
            (None, false) => self.tooltip(),
        };
        SaveState {
            off: signed_off || disagreement.is_some(),
            note: (signed_off && disagreement.is_none()).then(|| UNSIGNED_COPY.to_string()),
            tooltip,
        }
    }

    /// What [`Self::disagreement`] last said, for the close prompt and Save As's menu item.
    pub(crate) fn disagreement_now(&self) -> Option<String> {
        crate::lock(&self.disagreement).clone()
    }

    /// **What's known of this file's signatures,** as the save itself would find them (`signature_facts_of`, the fork's look through the bytes included), cached
    /// per version. Not known yet starts the look on a thread of its own, as it reads the whole file, and `done` is called when it has an answer; until then it's
    /// `None`. One look per file and version, and at most [`SIGNATURE_LOOKS`] at once: past that, `None` until one ends.
    pub(crate) fn signature_facts(
        &self,
        id: TabId,
        path: &std::path::Path,
        stamp: FileStamp,
        done: impl FnOnce() + Send + 'static,
    ) -> Option<recto_core::save::SignatureFacts> {
        self.signature_facts_by(id, path, stamp, recto_core::save::signature_facts_of, done)
    }

    /// [`Self::signature_facts`], with what does the looking given, so a test can make it slow or panic.
    fn signature_facts_by(
        &self,
        id: TabId,
        path: &std::path::Path,
        stamp: FileStamp,
        look: impl FnOnce(&std::path::Path) -> Result<recto_core::save::SignatureFacts, recto_core::OpenError> + Send + 'static,
        done: impl FnOnce() + Send + 'static,
    ) -> Option<recto_core::save::SignatureFacts> {
        if let Some((_, _, facts)) = crate::lock(&self.signature).iter().find(|(tab, at, _)| *tab == id && *at == stamp) {
            return Some(*facts);
        }
        let key = (id, stamp);
        let mut looking = crate::lock(&self.looking);
        if looking.contains(&key) || looking.len() >= SIGNATURE_LOOKS {
            return None;
        }
        looking.push(key);
        drop(looking);
        let ends = LookEnds {
            key,
            known: Arc::clone(&self.signature),
            looking: Arc::clone(&self.looking),
        };
        let path = path.to_path_buf();
        let spawned = std::thread::Builder::new()
            .name("recto-signatures".into())
            .stack_size(512 * 1024)
            .spawn(move || {
                let facts = look(&path).unwrap_or_default();
                remember(&ends.known, ends.key, facts);
                drop(ends);
                done();
            });
        if spawned.is_err() {
            crate::lock(&self.looking).retain(|looked| *looked != key);
        }
        None
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
    let copy_helps = matches!(
        error.code,
        "file_changed" | "read_only" | "hard_links" | "breaks_certification" | "locked_by_signature" | "signed_needs_copy" | "signed_original"
    );
    SaveProblem {
        title: "The document couldn't be saved.".into(),
        message: error.message.clone(),
        save_as_title: copy_helps.then(|| "Save As…".to_string()),
        ok_title: "OK".into(),
    }
}

/// Save As's sheet for the tab showing.
pub(crate) fn panel_for(tab: u32, name: &str, folder: &str, note: Option<String>) -> SavePanelModel {
    let stem = std::path::Path::new(name)
        .file_stem()
        .map(|stem| recto_core::clean_line(&stem.to_string_lossy()))
        .filter(|stem| !stem.is_empty());
    let suggested = stem.map_or_else(|| "Untitled.pdf".into(), |stem| format!("{stem} copy.pdf"));
    SavePanelModel {
        tab,
        prompt: "Save".into(),
        suggested_name: suggested,
        directory: folder.to_string(),
        message: note,
    }
}

/// What closing asks, about this document.
/// **What closing asks, decided here** (review): with unsaved edits, Save, Cancel or Don't Save, as any Mac app asks. Steps the person undid can't be redone
/// once the tab's closed, so it says so, and closing after either answer lets them go. With only undone steps, there's nothing to save: Close or Cancel.
/// `disagreement` is why neither Save nor Save As can save this file (the agreement gate): then only Cancel and Don't Save, with why.
pub(crate) fn close_prompt(tab: u32, name: &str, edited: bool, undone: bool, save_off: bool, disagreement: Option<&str>) -> ClosePrompt {
    let name = recto_core::clean_line(name);
    let button = |title: &str, choice, destructive| CloseButton {
        title: title.into(),
        choice,
        destructive,
    };
    if let Some(why) = disagreement.filter(|_| edited) {
        return ClosePrompt {
            tab,
            message: format!("Close “{name}” without saving the changes?"),
            informative: format!("{why} Your changes will be lost if you close it."),
            buttons: vec![button("Cancel", CloseChoice::Cancel, false), button("Don't Save", CloseChoice::DontSave, true)],
        };
    }
    if edited {
        let informative = if undone {
            "Your changes will be lost if you don't save them. Steps you undid can't be redone once it's closed."
        } else {
            "Your changes will be lost if you don't save them."
        };
        let save = if save_off {
            button("Save As…", CloseChoice::SaveAs, false)
        } else {
            button("Save", CloseChoice::Save, false)
        };
        let informative = if save_off {
            "This document is signed, so these changes can't be saved over it. Save As makes a new copy without the signature."
        } else {
            informative
        };
        ClosePrompt {
            tab,
            message: format!("Do you want to save the changes you made to “{name}”?"),
            informative: informative.into(),
            buttons: vec![
                save,
                button("Cancel", CloseChoice::Cancel, false),
                button("Don't Save", CloseChoice::DontSave, true),
            ],
        }
    } else {
        ClosePrompt {
            tab,
            message: format!("Close “{name}”?"),
            informative: "It has no unsaved changes, but steps you undid can't be redone once it's closed.".into(),
            buttons: vec![button("Close", CloseChoice::DontSave, true), button("Cancel", CloseChoice::Cancel, false)],
        }
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
        inner.save_ui.set_problem(Some(SaveProblem {
            title: "The document couldn't be saved.".into(),
            message: "The save couldn't be started, so nothing was written.".into(),
            save_as_title: None,
            ok_title: "OK".into(),
        }));
        inner.core.push(crate::CoreCall::Reconcile);
    }
}

/// The save of the tab the action was for, whichever is showing by now. A Save As's sheet has already asked whether to replace a file that's there, so one
/// that's there was agreed to; it's looked at here, off the UI thread (review).
fn run_save(inner: &crate::Inner, id: TabId, out: Option<std::path::PathBuf>, close_after: bool, answered: std::time::SystemTime) {
    use recto_control::host::Host;
    let replace = match out.as_deref().map(|out| replace_answered(out, answered)) {
        Some(Err(())) => {
            inner.save_ui.set_problem(Some(SaveProblem {
                title: "The document couldn't be saved.".into(),
                message: "A file appeared at that name after you chose it, so it wasn't replaced. Nothing was written.".into(),
                save_as_title: Some("Save As…".into()),
                ok_title: "OK".into(),
            }));
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

    /// Failure (review round 1 of stage 2): the Close prompt offers Save where Save is off, so the answer is a refusal. With Save off it offers Save As, and says why; otherwise Save, as before.
    #[test]
    fn the_close_prompt_offers_what_works() {
        let off = close_prompt(1, "signed.pdf", true, false, true, None);
        assert_eq!(
            off.buttons.iter().map(|button| button.choice).collect::<Vec<_>>(),
            [CloseChoice::SaveAs, CloseChoice::Cancel, CloseChoice::DontSave]
        );
        assert!(off.informative.contains("Save As makes a new copy"), "{}", off.informative);
        assert_eq!(close_prompt(1, "plain.pdf", true, false, false, None).buttons[0].choice, CloseChoice::Save);
    }

    /// Failure (the agreement gate): closing a file whose two readers disagree offers Save or Save As, each of which is refused, so the person is
    /// sent round in circles. It offers Cancel and Don't Save, and says why in the gate's words; with nothing unsaved it's the usual Close.
    #[test]
    fn the_close_prompt_offers_no_save_for_a_file_the_readers_disagree_about() {
        let why = recto_core::agreement::Disagreement::PageSize { page: 0 }.message();
        let prompt = close_prompt(1, "damaged.pdf", true, false, true, Some(&why));
        assert_eq!(
            prompt.buttons.iter().map(|button| button.choice).collect::<Vec<_>>(),
            [CloseChoice::Cancel, CloseChoice::DontSave]
        );
        assert!(
            prompt.informative.starts_with(&why) && prompt.informative.contains("will be lost"),
            "{}",
            prompt.informative
        );
        let unedited = close_prompt(1, "damaged.pdf", false, true, false, Some(&why));
        assert_eq!(unedited.buttons[0].choice, CloseChoice::DontSave, "nothing to save: Close");
    }

    /// Failure (SPEC decision 27): a save refused for a signature offers no way on, though Save As makes an unsigned copy and the original keeps its signature. The alert for a certified document, one a signature locks, a signed one whose change needs a rewrite, and Save As onto a signed file's own name (review round 2) each offers Save As; one for a damaged file doesn't, as a copy can't help it.
    #[test]
    fn a_signature_refusal_offers_save_as() {
        for code in ["breaks_certification", "locked_by_signature", "signed_needs_copy", "signed_original"] {
            let problem = problem_for(&recto_agent::AgentError::new(code, "refused", recto_agent::exit::FAILED));
            assert_eq!(problem.save_as_title.as_deref(), Some("Save As…"), "{code}");
        }
        assert_eq!(
            problem_for(&recto_agent::AgentError::new("damaged", "refused", recto_agent::exit::FAILED)).save_as_title,
            None
        );
    }

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

    /// Failure (review round 2): the signature look's one slot stays taken forever when the look panics, so Save is never offered as off for any file again; and one
    /// slow disk holds that slot for every tab. A look that panics ends, counting as having found nothing; a slow look on one file doesn't stop another's; and no
    /// more than the cap run at once.
    #[test]
    fn a_signature_look_ends_however_it_ends_and_one_file_doesnt_hold_the_rest() {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};
        let ui = SaveUi::new(Arc::new(AtomicU8::new(0)));
        let stamp = |n: u64| FileStamp {
            size: n,
            mtime_ns: n as i64,
            device: 1,
            inode: n,
        };
        let path = std::path::Path::new("/nowhere.pdf");
        let wait_for = |what: &dyn Fn() -> bool| {
            let started = Instant::now();
            while !what() {
                assert!(started.elapsed() < Duration::from_secs(10), "waited 10 s");
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        // A look that panics: its place is given back, and its answer is "nothing found".
        assert_eq!(ui.signature_facts_by(TabId(1), path, stamp(1), |_| panic!("a hostile file"), || {}), None);
        wait_for(&|| crate::lock(&ui.looking).is_empty());
        assert_eq!(
            ui.signature_facts_by(TabId(1), path, stamp(1), |_| unreachable!("answered already"), || {}),
            Some(Default::default())
        );
        // Slow looks on other files: each has its own place, up to the cap.
        let mut release = Vec::new();
        for n in 0..SIGNATURE_LOOKS as u64 {
            let (go, wait) = mpsc::channel::<()>();
            release.push(go);
            let look = move |_: &std::path::Path| {
                let _ = wait.recv();
                Ok(recto_core::save::SignatureFacts {
                    signed: true,
                    ..Default::default()
                })
            };
            assert_eq!(ui.signature_facts_by(TabId(10 + n as u32), path, stamp(10 + n), look, || {}), None);
        }
        assert_eq!(crate::lock(&ui.looking).len(), SIGNATURE_LOOKS, "one per file");
        assert_eq!(
            ui.signature_facts_by(TabId(99), path, stamp(99), |_| unreachable!("past the cap"), || {}),
            None,
            "past the cap, none starts"
        );
        assert_eq!(
            ui.signature_facts_by(TabId(10), path, stamp(10), |_| unreachable!("looked at already"), || {}),
            None,
            "and one file isn't looked at twice"
        );
        // Let them end: each answer is kept, and the places are free.
        release.remove(0).send(()).unwrap();
        drop(release);
        wait_for(&|| crate::lock(&ui.looking).is_empty());
        assert!(
            ui.signature_facts_by(TabId(10), path, stamp(10), |_| unreachable!("answered already"), || {})
                .is_some_and(|facts| facts.signed)
        );
    }
}
