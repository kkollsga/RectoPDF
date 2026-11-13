//! The search index as commands see it: where a document's corpus stands, what to do about it, and the words for that. The corpus itself is `recto_index::corpus`, and what builds it belongs to whoever holds the session, behind [`Indexing`], because this crate holds no state.

use crate::Code;
use crate::TabRef;
use crate::{AgentError, ConditionState, SessionAccess, exit, select};
use recto_index::corpus::{self, CorpusKey, DiskState, Summary};
use recto_platform::FileStamp;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

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

/// For a route that lives no longer than one command, such as `recto local`: it reads what's been built and starts nothing.
#[derive(Clone, Debug)]
pub struct ReadOnlyCorpus(pub PathBuf);

impl Indexing for ReadOnlyCorpus {
    fn root(&self) -> &Path {
        &self.0
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
        }
    }
}

fn refused_words(code: &str) -> &'static str {
    match code {
        "needs_password" => "it needs a password, and its text is never written to disk",
        "too_large" => "its text is more than an index may hold",
        "not_pdf" => "it couldn't be read as a PDF",
        "security" => "it uses a kind of security that can't be read",
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
    let disk = corpus::disk_state(indexing.root(), key);
    let pages_done = match &disk {
        DiskState::Ready(summary) => {
            // What it holds is good to use meanwhile: this only reads again pages that failed for a temporary reason.
            if want && summary.retry_is_due() {
                indexing.want(path, key);
            }
            return (Some(key), FileIndex::Ready(summary.clone()));
        }
        DiskState::Refused(code) => return (Some(key), FileIndex::Refused(code.clone())),
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
    match corpus::ensure_agent_folder(indexing.root(), key, &corpus::BuildLimits::default()) {
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
            FileIndex::Unavailable => {}
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
        "note": note,
    });
    (value, first_unready)
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

/// Waits for a build that's been asked to stop, a moment at most. If it hasn't, what's clearing says so.
fn wait_until_stopped(indexing: &dyn Indexing, key: Option<CorpusKey>) {
    let started = std::time::Instant::now();
    while started.elapsed() < STOP_WAIT && key.map_or_else(|| indexing.load().0 > 0, |key| indexing.job(key) == JobState::Building) {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// `recto index clear`: this tab's files, or everything, and whatever's on its way for them.
pub(crate) fn clear<A: SessionAccess>(access: &A, tab: &TabRef, all: bool) -> Result<Value, AgentError> {
    let indexing = access.indexing().ok_or_else(no_indexing)?;
    let failed = || AgentError::new(Code::WRITE_FAILED, "The index folder couldn't be cleared.", exit::FAILED);
    if all {
        indexing.cancel(None);
        wait_until_stopped(indexing, None);
        let report = corpus::clear_all(indexing.root()).map_err(|_| failed())?;
        let contents = access.generating().map(|generating| crate::generated::clear(generating, None));
        return said_cleared(report, contents);
    }
    let (_, files) = tab_files(access, tab)?;
    let mut report = corpus::Cleared::default();
    for file in &files {
        // Every version there's a record of, even if the file's gone, or the text of the last one stays behind what's there now.
        for key in corpus::keys_of_file(indexing.root(), &file.path) {
            // Before it's cleared, or the queue writes it all back.
            indexing.cancel(Some(key));
            wait_until_stopped(indexing, Some(key));
            report.note(corpus::clear(indexing.root(), key));
        }
        corpus::forget_file(indexing.root(), &file.path);
    }
    let paths: Vec<PathBuf> = files.into_iter().map(|file| file.path).collect();
    let contents = access.generating().map(|generating| crate::generated::clear(generating, Some(&paths)));
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
