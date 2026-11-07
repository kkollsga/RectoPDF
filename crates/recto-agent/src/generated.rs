//! Generated contents as commands see them: `recto contents --generated` and `recto wait contents-generated`. What makes them belongs to whoever holds the session, behind [`Generating`], because this crate holds no state. The format and the folder are `recto_index::generated`.
//!
//! **Made when asked, never on open** (SPEC decision 22's rule, which holds here too): asking for them is what starts the helper, and an answer that isn't ready says so at once and never waits.

use crate::{AgentError, ConditionState, SessionAccess, exit};
use recto_index::corpus::CorpusKey;
use recto_index::generated::{self, Generated};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Where one file's generated contents stand, as far as whoever makes them knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GenState {
    /// Nothing under way, and nothing known against it.
    Idle,
    Queued,
    Running,
    /// This version of the file won't have them, for this reason: it needs a password, or distillPDF's bounded engine won't read it.
    Refused(String),
    /// The last try failed for a reason that isn't about the file, or the helper was stopped at a limit. It can be asked for again after a pause.
    Failed(String),
}

/// How a session gets generated contents made. Nothing here may block: it's called from commands and the session thread, and making them is another thread's.
pub trait Generating: Send + Sync {
    /// Where they're kept.
    fn root(&self) -> &Path;
    /// Someone wants them for this file. Nothing happens if they're on their way, refused, or failed within the pause.
    fn want(&self, path: &Path, key: CorpusKey, pages: u32);
    fn state(&self, key: CorpusKey) -> GenState;
    /// Ones made for a file whose key can't be trusted on disk, held for this session only.
    fn held(&self, key: CorpusKey) -> Option<Arc<Generated>>;
    /// Stops what's on its way for this file, or for all.
    fn cancel(&self, key: Option<CorpusKey>);
}

/// One file's generated contents, if they've been made: in memory, or from disk.
pub fn find(generating: &dyn Generating, path: &Path, key: CorpusKey) -> Option<Arc<Generated>> {
    generating.held(key).or_else(|| generated::read(generating.root(), path, key).map(Arc::new))
}

/// The words for a refusal or a failure's code. Rust's own, with no text from a file.
pub fn reason_words(code: &str) -> &'static str {
    match code {
        "needs_password" => "it needs a password, and nothing of a protected file is read for this",
        "couldnt_read" => "distillPDF's bounded reader couldn't read it",
        "no_pages" => "it has no pages",
        "over_memory" => "finding them took more memory than they're allowed",
        "out_of_time" => "finding them took longer than they're allowed",
        "helper_missing" => "the helper that finds them isn't installed",
        "helper_stopped" => "the helper that finds them stopped without an answer",
        "bad_answer" => "the helper's answer didn't hold up",
        "couldnt_write" => "they couldn't be kept on disk",
        "changed" => "the file changed while they were being made",
        _ => "something went wrong making them",
    }
}

pub(crate) fn no_generating() -> AgentError {
    AgentError::new("no_generated_contents", "Generated contents aren't available in this session.", exit::FAILED).then("Run it against the app's session, or `recto serve`.")
}

/// The file a tab shows, and its key now, looked at with the session free.
pub(crate) struct Keyed {
    pub path: PathBuf,
    pub key: CorpusKey,
}

pub(crate) fn key_of(path: &Path) -> Result<CorpusKey, AgentError> {
    CorpusKey::of_file(path).map_err(|_| AgentError::new("file_unreadable", "The tab's file can't be looked at just now, so it has no generated contents.", exit::FAILED))
}

/// Where a file's generated contents stand, for an answer that isn't the contents: `None` once they're ready.
pub(crate) fn not_ready(generating: &dyn Generating, keyed: &Keyed, pages: u32, ask: bool) -> Option<Result<(&'static str, String), AgentError>> {
    if find(generating, &keyed.path, keyed.key).is_some() {
        return None;
    }
    Some(match generating.state(keyed.key) {
        GenState::Refused(code) => Err(AgentError::new("not_generated", format!("This file won't have generated contents: {}.", reason_words(&code)), if code == "needs_password" { exit::PROTECTED } else { exit::FAILED }).then("`recto contents` gives the file's own, if it has any.")),
        GenState::Failed(code) => Err(AgentError::new("generating_failed", format!("Generated contents couldn't be made: {}.", reason_words(&code)), exit::FAILED).then("Ask again in a minute.")),
        state => {
            if ask && state == GenState::Idle {
                generating.want(&keyed.path, keyed.key, pages);
            }
            let word = if generating.state(keyed.key) == GenState::Running { "running" } else { "queued" };
            Ok((word, "They're being made in the background, from the headings distillPDF finds. `recto wait contents-generated` waits for them.".to_string()))
        }
    })
}

/// `recto wait contents-generated`: asking counts as wanting them, or a wait could run out its time on something nobody started.
pub(crate) fn look<A: SessionAccess>(access: &A, tab: &str) -> ConditionState {
    let Some(generating) = access.generating() else { return ConditionState::Failed(no_generating()) };
    let (file, keyed) = match crate::reading::tab_file_keyed(access, tab) {
        Ok(found) => found,
        Err(error) if error.exit == exit::AMBIGUOUS => return ConditionState::Failed(error),
        Err(_) => return ConditionState::Waiting,
    };
    match not_ready(generating, &keyed, file.pages, true) {
        None => match crate::reading::contents(access, tab, None, None, None, true) {
            Ok(value) => ConditionState::Met(value),
            Err(error) => ConditionState::Failed(error),
        },
        Some(Err(error)) => ConditionState::Failed(error),
        Some(Ok(_)) => ConditionState::Waiting,
    }
}
