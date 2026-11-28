//! The search index as commands see it: where a document's corpus stands, what to do about it, and the words for that. The corpus itself is `recto_index::corpus`, and what builds it belongs to whoever holds the session, behind [`Indexing`], because this crate holds no state.

use crate::Code;
use crate::TabRef;
use crate::{AgentError, ConditionState, SessionAccess, exit, select};
use recto_index::corpus::{self, CorpusKey, DiskState, Summary};
use recto_platform::FileStamp;
use recto_platform::find_key::FindKey;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

mod key_use;
pub use key_use::{KeyUse, UseEnded};

/// What's being done about one file's corpus, as far as whoever builds them knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobState {
    /// Nothing, as far as this session goes.
    Idle,
    Queued,
    Building,
    /// The last try failed for a reason that isn't about the file's contents, such as an incomplete install. It'll be tried again after a pause.
    Failed(String),
}

/// How a session gets its documents indexed. Asking mustn't block: this is called from a command, and building is someone else's thread.
pub trait Indexing: Send + Sync {
    /// Where corpora live.
    fn root(&self) -> &Path;
    /// The key the text there is sealed with. `None` where this host keeps no index, and then nothing is read or built. **Hold the use for as long as what's opened with it can write:** a removal waits for every use to end.
    fn find_key(&self) -> Option<KeyUse<'_>>;
    /// An agent's folder is being made for this file's corpus, which lives only for this session: it's kept from the sweep, and set aside when the session ends.
    fn made_agent_folder(&self, _key: CorpusKey) {}
    /// What's known of the Find key the text is sealed with, without taking it: its plan, and where it came from once it has been. Never anything of the key.
    fn find_key_report(&self) -> Value {
        Value::Null
    }
    /// Someone wants this file indexed. Nothing happens if it's already on its way, and the oldest ask gives way if there are too many.
    fn want(&self, path: &Path, key: CorpusKey);
    fn job(&self, key: CorpusKey) -> JobState;
    /// Indexers running and files waiting, for anyone checking what's going on.
    fn load(&self) -> (u32, u32);
    /// Someone's using indexes in this session. The first time, it's the moment to clear what's past its time, since a deleted file's text may have nothing else to make that happen.
    fn note_asked(&self) {}
    /// Stops what's on its way for this file, or for all of them: takes it out of the queue, and if it's being built, stops that. What's written stays until something clears it.
    fn cancel(&self, _key: Option<CorpusKey>) {}
    /// Whether asking here gets anything built.
    fn starts_builds(&self) -> bool {
        true
    }
}

/// For a route that lives no longer than one command, such as `recto local`: it reads what's been built and starts nothing. With no key, it reads nothing either.
#[derive(Debug)]
pub struct ReadOnlyCorpus(pub PathBuf, pub Option<FindKey>);

impl Indexing for ReadOnlyCorpus {
    fn root(&self) -> &Path {
        &self.0
    }
    fn find_key(&self) -> Option<KeyUse<'_>> {
        self.1.as_ref().map(KeyUse::plain)
    }
    fn want(&self, _path: &Path, _key: CorpusKey) {}
    fn job(&self, _key: CorpusKey) -> JobState {
        JobState::Idle
    }
    fn load(&self) -> (u32, u32) {
        (0, 0)
    }
    fn starts_builds(&self) -> bool {
        false
    }
}

/// One file's corpus, as commands report it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FileIndex {
    Ready(Summary),
    Building {
        pages_done: u32,
    },
    Queued {
        pages_done: u32,
    },
    /// Nothing's been built, or a build stopped partway, and nothing is under way.
    NotStarted {
        pages_done: u32,
    },
    /// It won't be indexed, for this code's reason, while it's this version of the file.
    Refused(String),
    Failed(String),
    /// The file can't be looked at, or isn't what its page numbers were recorded from.
    Unavailable,
    /// There's no index for it just now, for a cause of the moment ([`told_to_agents`]): which one isn't said.
    NotNow,
}

/// **The one word an agent is told for a cause of the moment** (the coordinator's decision on the Find text's second review, item 7, 2026-11-15): the lasting key out of reach just now, a file that couldn't be opened or read just now, a renderer or helper not to be had just now, another session building it. All of them are `not_now`, so none can be told from another, and so the state of the person's Keychain can't be learned from an index. Causes about the file, or the install, that an agent can know or act on keep their own codes: a password, a size, a volume, not a PDF, a renderer missing. The cause itself stays in the session, and in `qa state` for the key.
pub const NOT_NOW: &str = "not_now";

const OF_THE_MOMENT: &[&str] = &[
    corpus::LASTING_KEY_OUT_OF_REACH,
    "couldnt_read",
    "couldnt_open",
    "not_found",
    "no_renderer",
    "render_unavailable",
    "too_busy",
    "out_of_time",
    "building_elsewhere",
    "indexer_wouldnt_start",
    "indexer_stopped",
    "indexer_panicked",
];

/// What an agent is told of a build's or a file's code: [`NOT_NOW`] for a cause of the moment, and the code itself for anything else.
#[must_use]
pub fn told_to_agents(code: &str) -> &str {
    if OF_THE_MOMENT.contains(&code) { NOT_NOW } else { code }
}

impl FileIndex {
    pub(crate) fn word(&self) -> &'static str {
        match self {
            Self::Ready(_) => "ready",
            Self::Building { .. } => "building",
            Self::Queued { .. } => "queued",
            Self::NotStarted { .. } => "not_started",
            Self::Refused(_) => "refused",
            Self::Failed(_) => "failed",
            Self::Unavailable => "unavailable",
            Self::NotNow => NOT_NOW,
        }
    }

    /// Where it stands, in plain words, for someone asking about the index rather than searching.
    fn status_note(&self, starts_builds: bool) -> String {
        match self {
            Self::Ready(_) => "Searches of this tab answer from the index.".to_string(),
            Self::Building { .. } | Self::Queued { .. } => {
                "It's on its way, in the background. `recto wait indexing-done` waits for it, and searches read the pages one by one until then.".to_string()
            }
            Self::NotStarted { .. } if starts_builds => "It isn't built yet. `recto index` or a search starts it.".to_string(),
            Self::NotStarted { .. } => "It isn't built, and this route doesn't build one. `recto index` in a session does.".to_string(),
            Self::Refused(code) => format!("This file isn't indexed: {}. Searches read the pages one by one.", refused_words(code)),
            Self::Failed(code) => format!("The index couldn't be built ({code}). Searches read the pages one by one, and it'll be tried again later."),
            Self::Unavailable => "A file behind this tab has changed or gone, so it can't be indexed as recorded.".to_string(),
            Self::NotNow => "There's no index for this file just now. Searches read the pages one by one, and a later run may have one.".to_string(),
        }
    }

    /// Why a search read pages one by one, in plain words. It says what to do about it, and it's what an agent sees.
    pub(crate) fn scan_note(&self, starts_builds: bool) -> String {
        match self {
            Self::Ready(_) => "Answered from the index.".to_string(),
            Self::Building { pages_done } => {
                format!("The index is being built ({pages_done} pages so far), so this read the pages one by one. `recto wait indexing-done` waits for it.")
            }
            Self::Queued { .. } => {
                "The index is waiting its turn to be built, so this read the pages one by one. `recto wait indexing-done` waits for it.".to_string()
            }
            Self::NotStarted { .. } if starts_builds => {
                "The index will be built next, so this read the pages one by one. `recto wait indexing-done` waits for it.".to_string()
            }
            Self::NotStarted { .. } => {
                "There's no index for this file, and this route doesn't build one, so it read the pages one by one. `recto index` in a session builds one."
                    .to_string()
            }
            Self::Refused(code) => format!("This file isn't indexed ({}), so this read the pages one by one.", refused_words(code)),
            Self::Failed(code) => format!("The index couldn't be built ({code}), so this read the pages one by one. It'll be tried again later."),
            Self::Unavailable => "This file can't be indexed as it is, so this read the pages one by one.".to_string(),
            Self::NotNow => "There's no index for this file just now, so this read the pages one by one. A later run may have one.".to_string(),
        }
    }
}

fn refused_words(code: &str) -> &'static str {
    match code {
        "needs_password" => "it needs a password, and its text is never written to disk",
        "too_large" => "its text is more than an index may hold",
        "not_pdf" => "it couldn't be read as a PDF",
        "security" => "it uses a kind of security that can't be read",
        "no_find_key" => "only the app keeps an index, sealed with its own key, and this program isn't the app",
        "coarse_file_times" => "it's on a volume whose file times can't show it's been replaced, so an index could answer from old text",
        _ => "it can't be read",
    }
}

/// Where a file's corpus stands, asking for it to be built if `want` says to.
pub(crate) fn file_index(indexing: &dyn Indexing, path: &Path, want: bool) -> (Option<CorpusKey>, FileIndex) {
    indexing.note_asked();
    let Ok(key) = CorpusKey::of_file(path) else {
        return (None, FileIndex::Unavailable);
    };
    if !corpus::can_be_indexed(path) {
        return (Some(key), FileIndex::Refused("coarse_file_times".to_string()));
    }
    let Some(find_key) = indexing.find_key() else {
        return (Some(key), FileIndex::Refused("no_find_key".to_string()));
    };
    let disk = corpus::disk_state(&find_key, indexing.root(), key);
    let pages_done = match &disk {
        DiskState::Ready(summary) => {
            // What it holds is good to use meanwhile: this only reads again pages that failed for a temporary reason.
            if want && summary.retry_is_due() {
                indexing.want(path, key);
            }
            return (Some(key), FileIndex::Ready(summary.clone()));
        }
        DiskState::Refused(code) => return (Some(key), FileIndex::Refused(code.clone())),
        // Sealed by the lasting key, which this run can't have just now: left as it is, and this file's search reads its pages.
        DiskState::SealedElsewhere { lasting: true } if !find_key.source().may_replace_lasting() => return (Some(key), FileIndex::NotNow),
        DiskState::SealedElsewhere { .. } => 0,
        DiskState::Partial { pages_done } => *pages_done,
        DiskState::Nothing => 0,
    };
    if want {
        indexing.want(path, key);
    }
    let state = match indexing.job(key) {
        JobState::Idle => FileIndex::NotStarted { pages_done },
        JobState::Queued => FileIndex::Queued { pages_done },
        JobState::Building => FileIndex::Building { pages_done },
        JobState::Failed(code) if told_to_agents(&code) == NOT_NOW => FileIndex::NotNow,
        JobState::Failed(code) => FileIndex::Failed(code),
    };
    (Some(key), state)
}

struct TabFile {
    path: PathBuf,
    /// As [`recto_session::recorded_version_is_current`] reads it.
    expected: Option<Option<FileStamp>>,
    added: bool,
    /// How many of the tab's pages come from it now. Only files with some matter to what's searched.
    pages_used: u32,
}

/// Every file a tab's pages come from.
fn tab_files<A: SessionAccess>(access: &A, tab: &TabRef) -> Result<(u32, Vec<TabFile>), AgentError> {
    access.with(|s| {
        let id = select(s, tab)?;
        let path = s
            .tabs()
            .path_of(id)
            .ok_or_else(|| AgentError::new(Code::NO_SUCH_TAB, "That tab isn't open.", exit::NOT_FOUND))?;
        let mut files = vec![TabFile {
            path,
            expected: s.notices().own_file_expected(id),
            added: false,
            pages_used: s.pages().count(id),
        }];
        if let Some(edits) = s.lists().get(id) {
            files[0].pages_used = 0;
            files.extend(edits.added_sources().iter().map(|added| TabFile {
                path: added.path.to_path_buf(),
                expected: Some(added.stamp),
                added: true,
                pages_used: 0,
            }));
            for slot in edits.slots() {
                if let Some(file) = files.get_mut(usize::from(slot.source)) {
                    file.pages_used += 1;
                }
            }
        }
        Ok((id.0, files))
    })
}

fn no_indexing() -> AgentError {
    AgentError::new(Code::NEEDS_SERVER, "Building an index needs the long-running session.", exit::NOT_RUNNING)
        .then("Start it with `recto serve --background`. `recto search` works without an index.")
}

/// Most files listed in an answer. A tab can hold many added PDFs, and all the rest get the same treatment.
const MAX_LISTED_FILES: usize = 32;

/// `recto index` and `recto index status`.
pub(crate) fn index<A: SessionAccess>(access: &A, tab: &TabRef, want: bool, agent: bool) -> Result<Value, AgentError> {
    let indexing = access.indexing().ok_or_else(no_indexing)?;
    let (id, files) = tab_files(access, tab)?;
    if agent {
        make_agent_folder(indexing, &files[0])?;
    }
    let answer = report(indexing, id, &files, want);
    Ok(answer.0)
}

/// Writes the tab's own file's folder for an agent, if its corpus is finished. If it isn't, there's nothing to make it from yet, and the answer says where that stands.
fn make_agent_folder(indexing: &dyn Indexing, file: &TabFile) -> Result<(), AgentError> {
    let current = recto_session::recorded_version_is_current(&file.path, file.expected);
    let (Some(key), FileIndex::Ready(_), true) = (CorpusKey::of_file(&file.path).ok(), file_index(indexing, &file.path, true).1, current) else {
        return Ok(());
    };
    let Some(find_key) = indexing.find_key() else {
        return Err(AgentError::new(
            Code::INDEX_FAILED,
            "This program keeps no index, so there's no folder to write from.",
            exit::FAILED,
        ));
    };
    // Before it's made: a sweep running meanwhile leaves it, and the session's end sets it aside.
    indexing.made_agent_folder(key);
    match corpus::ensure_agent_folder(&find_key, indexing.root(), key, &corpus::BuildLimits::default()) {
        Ok(_) | Err(corpus::AgentFolderError::NotReady) => Ok(()),
        Err(corpus::AgentFolderError::Busy) => Err(AgentError::new(Code::BUSY, "This document's index is being written right now.", exit::BUSY)),
        Err(corpus::AgentFolderError::TooLarge) => {
            Err(
                AgentError::new(Code::TOO_LARGE, "This document has too many pages for a folder of one file each.", exit::FAILED)
                    .then("`recto search` works without it."),
            )
        }
        Err(corpus::AgentFolderError::Damaged) => {
            let _ = corpus::clear(indexing.root(), key);
            indexing.want(&file.path, key);
            Err(AgentError::new(
                Code::INDEX_FAILED,
                "This document's index didn't add up, so it's being built again.",
                exit::FAILED,
            )
            .then("`recto wait indexing-done`, then ask again."))
        }
        Err(corpus::AgentFolderError::Failed) => Err(AgentError::new(Code::WRITE_FAILED, "The folder couldn't be written.", exit::FAILED)),
    }
}

/// What `index` and `wait` say. The second part is the first file, of the ones this tab uses, that isn't ready.
fn report(indexing: &dyn Indexing, id: u32, files: &[TabFile], want: bool) -> (Value, Option<FileIndex>) {
    let mut listed = Vec::new();
    let mut first_unready = None;
    let (mut pages, mut pages_done, mut ready, mut bytes, mut no_text, mut failed, mut flagged, mut unchecked) =
        (0u64, 0u64, 0u32, 0u64, 0u64, 0u64, 0u64, 0u64);
    let (mut folder, mut agent_bytes) = (None, 0);
    for file in files.iter().filter(|file| file.pages_used > 0) {
        let current = recto_session::recorded_version_is_current(&file.path, file.expected);
        let (key, state) = if current {
            file_index(indexing, &file.path, want)
        } else {
            (None, FileIndex::Unavailable)
        };
        let mut row = json!({ "added": file.added, "state": state.word(), "pages_used": file.pages_used });
        match &state {
            FileIndex::Ready(summary) => {
                ready += 1;
                (bytes, no_text, failed, flagged, unchecked) = (
                    bytes + summary.corpus_bytes,
                    no_text + u64::from(summary.no_text_pages),
                    failed + u64::from(summary.failed_pages),
                    flagged + u64::from(summary.hidden_flagged_pages),
                    unchecked + u64::from(summary.hidden_unchecked_pages),
                );
                (pages, pages_done) = (pages + u64::from(summary.pages), pages_done + u64::from(summary.pages));
                row["pages"] = json!(summary.pages);
                if let (false, Some(key)) = (file.added, key)
                    && let Some(bytes) = corpus::agent_folder_bytes(indexing.root(), key)
                {
                    folder = Some(corpus::CorpusPaths::new(indexing.root(), key).agent.to_string_lossy().into_owned());
                    agent_bytes = bytes;
                }
            }
            FileIndex::Building { pages_done: done } | FileIndex::Queued { pages_done: done } | FileIndex::NotStarted { pages_done: done } => {
                pages_done += u64::from(*done);
                row["pages_done"] = json!(done);
            }
            FileIndex::Refused(code) | FileIndex::Failed(code) => row["why"] = json!(code),
            FileIndex::Unavailable | FileIndex::NotNow => {}
        }
        if first_unready.is_none() && !matches!(state, FileIndex::Ready(_)) {
            first_unready = Some(state.clone());
        }
        if listed.len() < MAX_LISTED_FILES {
            if file.added {
                row["untrusted"] = json!({ "file": crate::file_label(&file.path) });
            }
            listed.push(row);
        }
    }
    let overall = first_unready.as_ref().map_or("ready", FileIndex::word);
    let note = first_unready.as_ref().map_or_else(
        || "Searches of this tab answer from the index.".to_string(),
        |state| state.status_note(indexing.starts_builds()),
    );
    let (running, waiting) = indexing.load();
    let value = json!({
        "tab": id,
        "state": overall,
        "files_ready": ready,
        "files": listed,
        "pages": pages,
        "pages_done": pages_done,
        "corpus_bytes": bytes,
        "no_text_pages": no_text,
        "failed_pages": failed,
        "hidden_flagged_pages": flagged,
        "hidden_unchecked_pages": unchecked,
        // Only once it's been asked for, and only the tab's own file. The folder's name says nothing about the document.
        "folder": folder,
        "agent_bytes": agent_bytes,
        "indexers_running": running,
        "indexers_waiting": waiting,
        // Where the key came from, and nothing of why: that's the person's Keychain's state, which an agent isn't told (the security review of slices 2 and 3, F8). `qa state` has the rest.
        "find_key": json!({ "source": source_told_to_agents(&indexing.find_key_report()["source"]) }),
        "note": note,
    });
    (value, first_unready)
}

/// Where the key came from, as an agent is told it: a key in memory only for now, because the Keychain refused this release just now, is a key in memory, as any other is (the second review's item 7 (b)): which it is would say the state of the person's Keychain.
fn source_told_to_agents(source: &Value) -> Value {
    match source.as_str() {
        Some("memory_for_now") => json!("memory"),
        _ => source.clone(),
    }
}

/// `recto wait indexing-done`: asking counts as wanting it, or a wait could run out its time on something nobody started.
pub(crate) fn look<A: SessionAccess>(access: &A, tab: &TabRef) -> ConditionState {
    let Some(indexing) = access.indexing() else {
        return ConditionState::Failed(no_indexing());
    };
    let (id, files) = match tab_files(access, tab) {
        Ok(found) => found,
        Err(error) if error.exit == exit::AMBIGUOUS => return ConditionState::Failed(error),
        Err(_) => return ConditionState::Waiting,
    };
    let (value, unready) = report(indexing, id, &files, true);
    match unready {
        None => ConditionState::Met(value),
        Some(FileIndex::Refused(code)) => ConditionState::Failed(
            AgentError::new(
                Code::NOT_INDEXED,
                format!("This file won't be indexed: {}.", refused_words(&code)),
                if code == "needs_password" { exit::PROTECTED } else { exit::FAILED },
            )
            .then("`recto search` still reads its pages one by one, if they can be read."),
        ),
        Some(FileIndex::Failed(code)) => ConditionState::Failed(
            AgentError::new(Code::INDEX_FAILED, format!("The index couldn't be built ({code})."), exit::FAILED)
                .then("`recto search` still reads the pages one by one. It's tried again after a pause."),
        ),
        Some(FileIndex::Unavailable) => ConditionState::Failed(
            AgentError::new(
                Code::INDEX_FAILED,
                "A file behind this tab has changed or gone, so it can't be indexed as recorded.",
                exit::FAILED,
            )
            .then("`recto edits` shows what's unsaved. `recto search` lists those pages as failed."),
        ),
        Some(_) => ConditionState::Waiting,
    }
}

/// How long a clear gives a build to stop. Its helper ends as soon as its stdin closes, and this runs on a job's thread.
const STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// Waits while `still` holds, a moment at most ([`STOP_WAIT`]): for a job that's been asked to stop to end. If it hasn't, what's clearing says so. The one wait every kind of job's stop goes by.
pub(crate) fn wait_while(still: impl Fn() -> bool) {
    let started = std::time::Instant::now();
    while started.elapsed() < STOP_WAIT && still() {
        std::thread::sleep(std::time::Duration::from_millis(5)); // ratchet: poll
    }
}

/// Waits for a build that's been asked to stop.
fn wait_until_stopped(indexing: &dyn Indexing, key: Option<CorpusKey>) {
    wait_while(|| key.map_or_else(|| indexing.load().0 > 0, |key| indexing.job(key) == JobState::Building));
}

/// `recto index clear`: this tab's files, or everything, and whatever's on its way for them.
pub(crate) fn clear<A: SessionAccess>(access: &A, tab: &TabRef, all: bool) -> Result<Value, AgentError> {
    let indexing = access.indexing().ok_or_else(no_indexing)?;
    let failed = || AgentError::new(Code::WRITE_FAILED, "The index folder couldn't be cleared.", exit::FAILED);
    if all {
        indexing.cancel(None);
        wait_until_stopped(indexing, None);
        if let Some(blocking) = access.blocking() {
            crate::blocks::stop(blocking, None);
        }
        let report = corpus::clear_all(indexing.root()).map_err(|_| failed())?;
        let contents = access.generating().map(|generating| crate::generated::clear(generating, None, &[]));
        return said_cleared(report, contents);
    }
    let (_, files) = tab_files(access, tab)?;
    if let Some(blocking) = access.blocking() {
        let paths: Vec<PathBuf> = files.iter().map(|file| file.path.clone()).collect();
        crate::blocks::stop(blocking, Some(&paths));
    }
    let mut report = corpus::Cleared::default();
    let mut all_keys = Vec::new();
    for file in &files {
        // Every version there's a record of, even if the file's gone, or the text of the last one stays behind what's there now.
        for key in corpus::keys_of_file(indexing.root(), &file.path) {
            // Before it's cleared, or the queue writes it all back.
            indexing.cancel(Some(key));
            wait_until_stopped(indexing, Some(key));
            report.note(corpus::clear(indexing.root(), key));
            all_keys.push(key);
        }
        corpus::forget_file(indexing.root(), &file.path);
    }
    let paths: Vec<PathBuf> = files.into_iter().map(|file| file.path).collect();
    let contents = access
        .generating()
        .map(|generating| crate::generated::clear(generating, Some(&paths), &all_keys));
    said_cleared(report, contents)
}

/// Says how a clear went, the search index's and generated contents', if this session makes them. Text that's still there is a failure: someone asked for it to be gone.
fn said_cleared(mut report: corpus::Cleared, contents: Option<corpus::Cleared>) -> Result<Value, AgentError> {
    let contents = contents.unwrap_or_default();
    report.failed += contents.failed;
    report.being_built += contents.being_built;
    if report.failed > 0 {
        return Err(AgentError::new(
            Code::CLEAR_INCOMPLETE,
            format!("{} couldn't be removed, so that text is still on disk. {} went.", report.failed, report.cleared),
            exit::FAILED,
        )
        .then("Tell the user. `recto index status` gives the folder, and they may need to remove it themselves."));
    }
    Ok(json!({ "cleared": report.cleared, "contents_cleared": contents.cleared, "being_built": report.being_built }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure (the second review's item 7, 2026-11-15): an agent learns the state of the person's Keychain from an index: the lasting key out of reach is told by a code of its own in a build's news or a file's row, or by where the key came from (`memory_for_now`, only ever a release the Keychain refused just now). Or a cause about the file, which an agent can know or act on, is hidden behind the same word. Each cause, and what an agent is told: the words take no level, so every level that may ask is told the same.
    #[test]
    fn a_cause_of_the_moment_is_one_word_to_an_agent() {
        let table = [
            ("lasting_key_out_of_reach", NOT_NOW),
            ("couldnt_read", NOT_NOW),
            ("couldnt_open", NOT_NOW),
            ("not_found", NOT_NOW),
            ("render_unavailable", NOT_NOW),
            ("too_busy", NOT_NOW),
            ("out_of_time", NOT_NOW),
            ("building_elsewhere", NOT_NOW),
            ("indexer_stopped", NOT_NOW),
            ("needs_password", "needs_password"),
            ("too_large", "too_large"),
            ("not_pdf", "not_pdf"),
            ("security", "security"),
            ("coarse_file_times", "coarse_file_times"),
            ("render_worker_missing", "render_worker_missing"),
            ("render_library_missing", "render_library_missing"),
            ("indexer_missing", "indexer_missing"),
            ("no_find_key", "no_find_key"),
        ];
        for (cause, told) in table {
            assert_eq!(told_to_agents(cause), told, "{cause}");
        }
        for source in ["keychain", "state_file", "memory"] {
            assert_eq!(source_told_to_agents(&json!(source)), json!(source), "{source}");
        }
        assert_eq!(
            source_told_to_agents(&json!("memory_for_now")),
            json!("memory"),
            "a key only for now was told as such"
        );
        assert_eq!(source_told_to_agents(&Value::Null), Value::Null);
    }
}
