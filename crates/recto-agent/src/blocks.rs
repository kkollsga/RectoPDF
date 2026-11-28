//! Blocks as commands see them: `recto get --grain paragraph|section|chapter` and `recto wait blocks-ready`. What makes them belongs to whoever holds the session,
//! behind [`Blocking`], because this crate holds no state; the store and the format are `recto_index::corpus::blocks` (`docs/design/blocks.md`).
//!
//! **Made when asked, never on open, and never by the window:** the first `get` of a blocks grain, or a `wait blocks-ready`, is what asks. An ask is made once: while a build is
//! queued or running, or after it was refused or failed, asking again starts nothing. Nothing here waits: one key, one ready-file read, one header open.

use crate::generated::GenState;
use crate::{AgentError, Code, ConditionState, SessionAccess, TabRef, exit};
use recto_index::blocks::map::Shape;
use recto_index::corpus::{self, CorpusKey, DiskState, ReadyCorpus, StoredBlocks};
use recto_platform::find_key::FindKey;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// What `get` says of a blocks grain it can't give: nothing is made for this file yet.
pub(crate) const NOT_MADE: &str = "blocks_not_made";
/// What `get` says while the blocks are on their way.
pub(crate) const BUILDING: &str = "blocks_building";

/// One document whose blocks someone wants.
pub struct BlocksAsk {
    pub path: PathBuf,
    pub key: CorpusKey,
    pub corpus_root: PathBuf,
    /// A key bound to this one corpus (`recto_index::seal::grant_for`), which is all the build may open.
    pub grant: FindKey,
}

/// How a session gets blocks made. Nothing here may block: it's called from commands, and making them is another thread's. The state is [`GenState`], as generated contents' is.
pub trait Blocking: Send + Sync {
    /// Someone wants blocks for this file. Nothing happens if they're on their way, refused, or failed within the pause.
    fn want(&self, ask: BlocksAsk);
    fn state(&self, key: CorpusKey) -> GenState;
    /// Stops what's on its way for these files, by path, every version, or for every file.
    fn cancel_files(&self, paths: Option<&[PathBuf]>);
    /// Whether one of these files', or any file's, is being made now.
    fn making(&self, paths: Option<&[PathBuf]>) -> bool;
}

/// **`recto index clear` stops blocks first:** a running build holds the corpus's `build.lock`, so without this the folder survives as being built. What's queued is taken off, a
/// running one is told to stop and waited for a moment.
pub(crate) fn stop(blocking: &dyn Blocking, paths: Option<&[PathBuf]>) {
    blocking.cancel_files(paths);
    crate::index::wait_while(|| blocking.making(paths));
}

/// What a corpus has of blocks, for a file whose index is ready: the stored ones made against this corpus as it stands now (its pages and its retries), if they open. A set that doesn't add up is
/// thrown away.
fn stored_pair(find_key: &FindKey, root: &Path, key: CorpusKey) -> Option<(ReadyCorpus, StoredBlocks)> {
    let ready = ReadyCorpus::open(find_key, root, key).ok()??;
    match StoredBlocks::open_matching(find_key, root, key, &ready.summary) {
        Ok(found) => found.map(|found| (ready, found)),
        Err(_) => {
            corpus::remove_blocks(root, key);
            None
        }
    }
}

fn stored(find_key: &FindKey, root: &Path, key: CorpusKey) -> Option<StoredBlocks> {
    stored_pair(find_key, root, key).map(|(_, found)| found)
}

/// The stored blocks of this file as it is now, if there are some. **Never asks for them:** a read that only uses them where they are (`get` of a hit) starts nothing.
pub(crate) fn stored_now<A: SessionAccess>(access: &A, key: CorpusKey) -> Option<StoredBlocks> {
    let indexing = access.indexing()?;
    stored(indexing.find_key()?, indexing.root(), key)
}

/// Asks for blocks if nobody has, and says where they stand.
fn ask(blocking: &dyn Blocking, find_key: &FindKey, root: &Path, path: &Path, key: CorpusKey) -> GenState {
    let state = blocking.state(key);
    if state != GenState::Idle {
        return state;
    }
    if let Some(grant) = recto_index::seal::grant_for(find_key, key) {
        blocking.want(BlocksAsk {
            path: path.to_path_buf(),
            key,
            corpus_root: root.to_path_buf(),
            grant,
        });
    }
    blocking.state(key)
}

/// What a blocks grain has to read from: the finished corpus and its blocks, or why the page is answered.
pub(crate) enum Readable {
    Ready(Box<(ReadyCorpus, StoredBlocks)>),
    /// The page is answered, with this `why` and, when blocks were refused or failed, Rust's words for it.
    Widen {
        why: &'static str,
        note: Option<&'static str>,
    },
}

/// **What a blocks grain can read:** the stored blocks if there are some for this file as it is now. Otherwise the `why` it widens to, and the first ask asks for them to be made: [`BUILDING`] while
/// they're on their way, [`NOT_MADE`] otherwise, with the words for why when they were refused or failed. A set found stale or damaged is thrown away and asked for again.
pub(crate) fn readable<A: SessionAccess>(access: &A, path: &Path) -> Readable {
    let not_made = Readable::Widen { why: NOT_MADE, note: None };
    let (Some(indexing), Some(blocking)) = (access.indexing(), access.blocking()) else {
        return not_made;
    };
    let Some(find_key) = indexing.find_key() else { return not_made };
    let Ok(key) = CorpusKey::of_file(path) else { return not_made };
    if !matches!(corpus::disk_state(find_key, indexing.root(), key), DiskState::Ready(_)) {
        return not_made;
    }
    if let Some(pair) = stored_pair(find_key, indexing.root(), key) {
        return Readable::Ready(Box::new(pair));
    }
    match ask(blocking, find_key, indexing.root(), path, key) {
        GenState::Queued | GenState::Running => Readable::Widen { why: BUILDING, note: None },
        GenState::Refused(code) | GenState::Failed(code) => Readable::Widen {
            why: NOT_MADE,
            note: Some(reason_words(&code)),
        },
        GenState::Idle => not_made,
    }
}

/// Blocks read as stale or damaged after they opened: taken away, asked for again, and what the answer widens to meanwhile.
pub(crate) fn throw_away_and_ask<A: SessionAccess>(access: &A, path: &Path) -> (&'static str, Option<&'static str>) {
    if let (Some(indexing), Ok(key)) = (access.indexing(), CorpusKey::of_file(path)) {
        corpus::remove_blocks(indexing.root(), key);
    }
    match readable(access, path) {
        Readable::Widen { why, note } => (why, note),
        // They read after all (the removal was skipped, as a build holds the folder): they are on their way if that build is queued or running, and otherwise not made.
        Readable::Ready(_) => match (access.blocking(), CorpusKey::of_file(path)) {
            (Some(blocking), Ok(key)) if matches!(blocking.state(key), GenState::Queued | GenState::Running) => (BUILDING, None),
            _ => (NOT_MADE, None),
        },
    }
}

/// The words for a refusal or a failure's code. Rust's own, with no text from a file.
pub fn reason_words(code: &str) -> &'static str {
    REASONS
        .iter()
        .find(|(known, _)| *known == code)
        .map_or("something went wrong making them", |(_, words)| words)
}

/// The helper's, the store's and the index's own reasons, each with its words.
const REASONS: &[(&str, &str)] = &[
    ("needs_password", "it needs a password, and nothing of a protected file is read for this"),
    ("couldnt_read", "distillPDF's bounded reader couldn't read it"),
    ("no_pages", "it has no pages"),
    ("pages_differ", "the helper and the index count different pages in it"),
    ("too_large", "its blocks and its text together are more than one document may take"),
    ("over_memory", "finding them took more memory than they're allowed"),
    ("out_of_time", "finding them took longer than they're allowed"),
    ("helper_missing", "the helper that finds them isn't installed"),
    ("helper_stopped", "the helper that finds them stopped without an answer"),
    ("bad_answer", "the helper's answer didn't hold up"),
    (
        "structure_not_sandboxed",
        "the helper that reads blocks isn't in its sandbox, as this copy is signed wrong",
    ),
    (
        "structure_inside_another_sandbox",
        "the helper that reads blocks can't start inside another sandbox",
    ),
    (
        "structure_walls_failed",
        "the helper that reads blocks couldn't close itself off from the rest of macOS",
    ),
    ("structure_start_refused", "macOS refused to start the helper that reads blocks"),
    (
        "structure_unconfirmed",
        "the helper that reads blocks couldn't be confirmed walled off, so the file wasn't read",
    ),
    ("busy", "the file's index is being written right now"),
    ("index_not_ready", "the file's index isn't finished"),
    ("index_damaged", "the file's index didn't add up; `recto index` builds it again"),
    ("write_failed", "they couldn't be kept on disk"),
];

/// The reason that is the person's to act on: the file needs a password, which an agent is never given.
const NEEDS_PASSWORD: &str = "needs_password";

fn no_blocking() -> AgentError {
    AgentError::new(Code::NEEDS_SERVER, "Blocks aren't made in this session.", exit::NOT_RUNNING).then("Run it against the app's session, or `recto serve`.")
}

/// A stored set as `wait blocks-ready` gives it: numbers only.
fn ready_json(tab: u32, made: &corpus::BlocksMade) -> Value {
    let stored: u64 = made.stored.iter().map(|count| u64::from(*count)).sum();
    let dropped: u64 = made.dropped.iter().map(|count| u64::from(*count)).sum::<u64>() + u64::from(made.unreadable);
    let by_kind: serde_json::Map<String, Value> = Shape::ALL
        .iter()
        .map(|shape| {
            (
                shape.name().to_string(),
                json!({ "stored": made.stored[shape.index()], "dropped": made.dropped[shape.index()] }),
            )
        })
        .collect();
    json!({
        "tab": tab, "state": "ready", "pages": made.pages, "stored": stored, "dropped": dropped, "bytes": made.bytes,
        "bytes_per_page": made.bytes.checked_div(u64::from(made.pages)).unwrap_or(0), "by_kind": by_kind,
    })
}

/// `recto wait blocks-ready`: asking counts as wanting the index, and then the blocks, or a wait could run out its time on something nobody started.
pub(crate) fn look<A: SessionAccess>(access: &A, tab: &TabRef) -> ConditionState {
    match crate::index::look(access, tab) {
        ConditionState::Met(_) => {}
        other => return other,
    }
    let (Some(indexing), Some(blocking)) = (access.indexing(), access.blocking()) else {
        return ConditionState::Failed(no_blocking());
    };
    let Some(find_key) = indexing.find_key() else {
        return ConditionState::Failed(no_blocking());
    };
    let (file, keyed) = match crate::reading::tab_file_keyed(access, tab) {
        Ok(found) => found,
        Err(error) if error.code == Code::NO_SUCH_TAB && error.exit != exit::AMBIGUOUS => return ConditionState::Waiting,
        Err(error) => return ConditionState::Failed(error),
    };
    if let Some(found) = stored(find_key, indexing.root(), keyed.key) {
        return ConditionState::Met(ready_json(file.id(), found.made()));
    }
    match ask(blocking, find_key, indexing.root(), &keyed.path, keyed.key) {
        GenState::Refused(code) => ConditionState::Failed(AgentError::new(
            Code::BLOCKS_REFUSED,
            format!("This file won't have blocks: {}.", reason_words(&code)),
            if code == NEEDS_PASSWORD { exit::PROTECTED } else { exit::FAILED },
        )),
        GenState::Failed(code) => ConditionState::Failed(AgentError::new(
            Code::BLOCKS_FAILED,
            format!("Blocks couldn't be made: {}. Ask again in a minute.", reason_words(&code)),
            exit::FAILED,
        )),
        GenState::Idle | GenState::Queued | GenState::Running => ConditionState::Waiting,
    }
}

#[cfg(test)]
mod tests;
