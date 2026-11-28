//! Blocks as commands see them: `recto get --grain paragraph|section|chapter` and `recto wait blocks-ready`. What makes them belongs to whoever holds the session,
//! behind [`Blocking`], because this crate holds no state; the store and the format are `recto_index::corpus::blocks` (`docs/design/blocks.md`).
//!
//! **Made when asked, never on open, and never by the window:** the first `get` of a blocks grain, or a `wait blocks-ready`, is what asks. An ask is made once: while a build is
//! queued or running, or after it was refused or failed, asking again starts nothing. Nothing here waits: one key, one ready-file read, one header open.

use crate::generated::GenState;
use crate::{AgentError, Code, ConditionState, SessionAccess, TabRef, exit};
use recto_index::blocks::map::Shape;
use recto_index::corpus::{self, CorpusKey, DiskState, StoredBlocks};
use recto_platform::find_key::FindKey;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

/// How long a clear gives a running build to stop. It ends as soon as it's told, and a stopped one writes nothing.
const STOP_WAIT: Duration = Duration::from_secs(3);

/// **`recto index clear` stops blocks first:** a running build holds the corpus's `build.lock`, so without this the folder survives as being built. What's queued is taken off, a
/// running one is told to stop and waited for a moment.
pub(crate) fn stop(blocking: &dyn Blocking, paths: Option<&[PathBuf]>) {
    blocking.cancel_files(paths);
    let started = Instant::now();
    while started.elapsed() < STOP_WAIT && blocking.making(paths) {
        std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
    }
}

/// What a corpus has of blocks, for a file whose index is ready: the stored ones, if they open. A set that doesn't add up is thrown away.
fn stored(find_key: &FindKey, root: &Path, key: CorpusKey) -> Option<StoredBlocks> {
    match StoredBlocks::open(find_key, root, key) {
        Ok(found) => found,
        Err(_) => {
            corpus::remove_blocks(root, key);
            None
        }
    }
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

/// **The `why` of a blocks grain answered as a page:** asks for the blocks if this is the first ask, and says [`BUILDING`] while they're on their way. Until blocks are read back (S5),
/// everything else is [`NOT_MADE`], made or not.
pub(crate) fn why_page<A: SessionAccess>(access: &A, path: &Path) -> &'static str {
    let (Some(indexing), Some(blocking)) = (access.indexing(), access.blocking()) else {
        return NOT_MADE;
    };
    let Some(find_key) = indexing.find_key() else { return NOT_MADE };
    let Ok(key) = CorpusKey::of_file(path) else { return NOT_MADE };
    if !matches!(corpus::disk_state(find_key, indexing.root(), key), DiskState::Ready(_)) || stored(find_key, indexing.root(), key).is_some() {
        return NOT_MADE;
    }
    match ask(blocking, find_key, indexing.root(), path, key) {
        GenState::Queued | GenState::Running => BUILDING,
        _ => NOT_MADE,
    }
}

/// The words for a refusal or a failure's code. Rust's own, with no text from a file.
pub fn reason_words(code: &str) -> &'static str {
    match code {
        "needs_password" => "it needs a password, and nothing of a protected file is read for this",
        "couldnt_read" => "distillPDF's bounded reader couldn't read it",
        "no_pages" => "it has no pages",
        "pages_differ" => "the helper and the index count different pages in it",
        "too_large" => "its blocks and its text together are more than one document may take",
        "over_memory" => "finding them took more memory than they're allowed",
        "out_of_time" => "finding them took longer than they're allowed",
        "helper_missing" => "the helper that finds them isn't installed",
        "helper_stopped" => "the helper that finds them stopped without an answer",
        "bad_answer" => "the helper's answer didn't hold up",
        "structure_not_sandboxed" => "the helper that reads blocks isn't in its sandbox, as this copy is signed wrong",
        "structure_inside_another_sandbox" => "the helper that reads blocks can't start inside another sandbox",
        "structure_walls_failed" => "the helper that reads blocks couldn't close itself off from the rest of macOS",
        "structure_start_refused" => "macOS refused to start the helper that reads blocks",
        "structure_unconfirmed" => "the helper that reads blocks couldn't be confirmed walled off, so the file wasn't read",
        "busy" => "the file's index is being written right now",
        "index_not_ready" => "the file's index isn't finished",
        "index_damaged" => "the file's index didn't add up, so it's being built again",
        "write_failed" => "they couldn't be kept on disk",
        _ => "something went wrong making them",
    }
}

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
            if code == "needs_password" { exit::PROTECTED } else { exit::FAILED },
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
