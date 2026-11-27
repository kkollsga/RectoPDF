//! Generated contents as commands see them: `recto contents --generated` and `recto wait contents-generated`. What makes them belongs to whoever holds the session, behind [`Generating`], because this crate holds no state. The format and the folder are `recto_index::generated`.
//!
//! **Made when asked, never on open** (SPEC decision 22's rule, which holds here too): asking for them is what starts the helper, and an answer that isn't ready says so at once and never waits.

use crate::Code;
use crate::TabRef;
use crate::{AgentError, ConditionState, SessionAccess, exit};
use recto_index::corpus::{Cleared, CorpusKey};
use recto_index::generated::{self, Generated};
use recto_session::GeneratedView;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    /// Stops what's on its way for these files, by path, every version, or for every file.
    fn cancel_files(&self, paths: Option<&[PathBuf]>);
    /// Whether one of these files', or any file's, is being made now.
    fn making(&self, paths: Option<&[PathBuf]>) -> bool;
}

/// How long a clear gives a running helper to stop. It ends as soon as it's told, and a stopped one writes nothing.
const STOP_WAIT: Duration = Duration::from_secs(3);

/// **`recto index clear` takes generated contents too:** headings are the file's text, kept on disk, and the command says it removes that. These files', or with `None` every file's. What's on its way for them stops first, and a running one is waited for a moment, so nothing's written back after. One still running after that is counted as being built.
pub(crate) fn clear(generating: &dyn Generating, paths: Option<&[PathBuf]>) -> Cleared {
    generating.cancel_files(paths);
    let started = Instant::now();
    while started.elapsed() < STOP_WAIT && generating.making(paths) {
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut report = match paths {
        None => generated::clear_all(generating.root()),
        Some(paths) => paths
            .iter()
            .map(|path| generated::clear(generating.root(), path))
            .fold(Cleared::default(), |sum, one| Cleared {
                cleared: sum.cleared + one.cleared,
                being_built: sum.being_built + one.being_built,
                failed: sum.failed + one.failed,
            }),
    };
    report.being_built += u32::from(generating.making(paths));
    report
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
        "structure_not_sandboxed" => "the helper that reads headings isn't in its sandbox, as this copy is signed wrong",
        "structure_inside_another_sandbox" => "the helper that reads headings can't start inside another sandbox",
        "structure_walls_failed" => "the helper that reads headings couldn't close itself off from the rest of macOS",
        "structure_start_refused" => "macOS refused to start the helper that reads headings",
        "couldnt_write" => "they couldn't be kept on disk",
        "changed" => "the file changed while they were being made",
        _ => "something went wrong making them",
    }
}

/// The code a failed job is told by: one of the four that say the structure helper couldn't be started walled, each with its own next step, and `generating_failed` for any other.
fn failed_code(code: &str) -> Code {
    match Code::listed(code) {
        Some(
            listed @ (Code::STRUCTURE_INSIDE_ANOTHER_SANDBOX | Code::STRUCTURE_NOT_SANDBOXED | Code::STRUCTURE_START_REFUSED | Code::STRUCTURE_WALLS_FAILED),
        ) => listed,
        _ => Code::GENERATING_FAILED,
    }
}

pub(crate) fn no_generating() -> AgentError {
    AgentError::new(
        Code::NO_GENERATED_CONTENTS,
        "Generated contents aren't available in this session.",
        exit::FAILED,
    )
    .then("Run it against the app's session, or `recto serve`.")
}

/// The file a tab shows, and its key now, looked at with the session free.
pub(crate) struct Keyed {
    pub path: PathBuf,
    pub key: CorpusKey,
}

pub(crate) fn key_of(path: &Path) -> Result<CorpusKey, AgentError> {
    CorpusKey::of_file(path).map_err(|_| {
        AgentError::new(
            Code::FILE_UNREADABLE,
            "The tab's file can't be looked at just now, so it has no generated contents.",
            exit::FAILED,
        )
    })
}

/// Where a file's generated contents stand, for an answer that isn't the contents: `None` once they're ready.
pub(crate) fn not_ready(generating: &dyn Generating, keyed: &Keyed, pages: u32, ask: bool) -> Option<Result<(&'static str, String), AgentError>> {
    if find(generating, &keyed.path, keyed.key).is_some() {
        return None;
    }
    Some(match generating.state(keyed.key) {
        GenState::Refused(code) => Err(AgentError::new(
            Code::NOT_GENERATED,
            format!("This file won't have generated contents: {}.", reason_words(&code)),
            if code == "needs_password" { exit::PROTECTED } else { exit::FAILED },
        )
        .then("`recto contents` gives the file's own, if it has any.")),
        GenState::Failed(code) => {
            let which = failed_code(&code);
            let error = AgentError::new(which, format!("Generated contents couldn't be made: {}.", reason_words(&code)), exit::FAILED);
            // The helper's start failing for a reason a minute won't change says what to do about it itself (its row in the table); only the others are asked for again.
            Err(if which == Code::GENERATING_FAILED {
                error.then("Ask again in a minute.")
            } else {
                error
            })
        }
        state => {
            if ask && state == GenState::Idle {
                generating.want(&keyed.path, keyed.key, pages);
            }
            let word = if generating.state(keyed.key) == GenState::Running {
                "running"
            } else {
                "queued"
            };
            Ok((
                word,
                "They're being made in the background, from the headings distillPDF finds. `recto wait contents-generated` waits for them.".to_string(),
            ))
        }
    })
}

/// **What the sidebar's note shows for a file's generated contents now,** and asked for if `ask`, or `None` if it's still what it was: made (or made with no
/// headings to list), on their way, refused or failed, or not kept. It looks at the file, for its key, and at the folder they're kept in, so it's called with
/// the session free, and never on the window's session thread or the UI's: a file on a dead volume can hold a look as long as it likes.
pub fn where_they_stand(generating: &dyn Generating, path: &Path, pages: u32, ask: bool) -> Option<GeneratedView> {
    let Ok(key) = CorpusKey::of_file(path) else {
        return Some(GeneratedView::Failed("The file can't be looked at just now."));
    };
    if let Some(made) = find(generating, path, key) {
        // A scan with no text layer, or a document set all in one size, has nothing to list: said, rather than shown as an empty list.
        if made.entries.is_empty() {
            return Some(GeneratedView::NoneFound);
        }
        return Some(GeneratedView::Ready(Arc::new(made.toc())));
    }
    if ask {
        generating.want(path, key, pages);
    }
    match generating.state(key) {
        GenState::Queued | GenState::Running => ask.then_some(GeneratedView::Making),
        GenState::Refused(code) | GenState::Failed(code) => Some(GeneratedView::Failed(failure_words(&code))),
        GenState::Idle => Some(if ask { GeneratedView::Making } else { GeneratedView::NotKept }),
    }
}

/// Why, for the pointer resting on the note. Rust's words.
pub fn failure_words(code: &str) -> &'static str {
    match code {
        "needs_password" => "This file needs a password, and nothing of a protected file is read for this.",
        "couldnt_read" | "no_pages" => "distillPDF couldn't read this file.",
        "over_memory" => "Finding the headings took more memory than it's allowed.",
        "out_of_time" => "Finding the headings took longer than it's allowed.",
        "helper_missing" => "The helper that finds headings isn't installed.",
        "structure_not_sandboxed" => "This copy of RectoPDF is signed wrong, so it won't read headings. Installing it again from a release fixes it.",
        "structure_inside_another_sandbox" => "RectoPDF can't read headings from inside another sandbox. Nothing is wrong with the install.",
        "structure_walls_failed" => "RectoPDF's heading reader couldn't close itself off on this version of macOS, so it won't read the file.",
        "structure_start_refused" => "macOS refused to start RectoPDF's heading reader. A security tool may be blocking it.",
        _ => "Something went wrong finding the headings. Click to try again.",
    }
}

/// Whether a tab a wait asks about isn't open yet, so it may be: no tab by that name, which an ambiguous name isn't.
fn not_open_yet(error: &AgentError) -> bool {
    error.code == Code::NO_SUCH_TAB && error.exit != exit::AMBIGUOUS
}

/// `recto wait contents-generated`: asking counts as wanting them, or a wait could run out its time on something nobody started.
pub(crate) fn look<A: SessionAccess>(access: &A, tab: &TabRef) -> ConditionState {
    let Some(generating) = access.generating() else {
        return ConditionState::Failed(no_generating());
    };
    let (file, keyed) = match crate::reading::tab_file_keyed(access, tab) {
        Ok(found) => found,
        // A tab that isn't open yet may be; a file that's changed under its edits or can't be looked at won't be, so the wait ends now rather than at its time.
        Err(error) if not_open_yet(&error) => return ConditionState::Waiting,
        Err(error) => return ConditionState::Failed(error),
    };
    match not_ready(generating, &keyed, file.pages(), true) {
        None => match crate::reading::contents(access, tab, None, None, None, true) {
            Ok(value) => ConditionState::Met(value),
            Err(error) => ConditionState::Failed(error),
        },
        Some(Err(error)) => ConditionState::Failed(error),
        Some(Ok(_)) => ConditionState::Waiting,
    }
}

#[cfg(test)]
mod tests {
    use super::failure_words;

    /// Failure (R2.5): a wait for generated contents on a tab not open yet ends at once as failed, so an agent opening it next waits for nothing; or one on
    /// a name that matches two tabs, or a file changed under its edits, waits out its time. Routed by a named code: only no such tab waits.
    #[test]
    fn only_a_tab_not_open_yet_is_waited_for() {
        use super::{AgentError, Code, exit, not_open_yet};
        assert!(not_open_yet(&AgentError::new(Code::NO_SUCH_TAB, "That tab isn't open.", exit::NOT_FOUND)));
        assert!(!not_open_yet(&AgentError::new(
            Code::AMBIGUOUS,
            "More than one tab matches that name.",
            exit::AMBIGUOUS
        )));
        assert!(!not_open_yet(&AgentError::new(Code::FILE_CHANGED, "changed", exit::FAILED)));
    }

    /// A session whose one file's job failed with `code`.
    struct Failed(&'static str);

    impl super::Generating for Failed {
        fn root(&self) -> &std::path::Path {
            std::path::Path::new("/nonexistent/recto-generated")
        }
        fn want(&self, _: &std::path::Path, _: super::CorpusKey, _: u32) {}
        fn state(&self, _: super::CorpusKey) -> super::GenState {
            super::GenState::Failed(self.0.into())
        }
        fn held(&self, _: super::CorpusKey) -> Option<std::sync::Arc<super::Generated>> {
            None
        }
        fn cancel(&self, _: Option<super::CorpusKey>) {}
        fn cancel_files(&self, _: Option<&[std::path::PathBuf]>) {}
        fn making(&self, _: Option<&[std::path::PathBuf]>) -> bool {
            false
        }
    }

    /// Failure: a helper that couldn't start walled is told as any other failure, "ask again in a minute", so an agent asks again and again for something a minute won't change; or its own
    /// next step (that the copy is signed wrong, that a sandbox is in the way) is lost. Each of the four gets its own code and the table's next step, any other failure stays `generating_failed`.
    #[test]
    fn a_helper_that_could_not_start_walled_is_told_by_its_own_code() {
        use super::{Code, Keyed, not_ready};
        use crate::code::Next;
        let key = super::CorpusKey::from_hex(&"ab".repeat(32)).unwrap();
        let keyed = Keyed { path: "/d/a.pdf".into(), key };
        for (reason, code) in [
            ("structure_not_sandboxed", Code::STRUCTURE_NOT_SANDBOXED),
            ("structure_inside_another_sandbox", Code::STRUCTURE_INSIDE_ANOTHER_SANDBOX),
            ("structure_walls_failed", Code::STRUCTURE_WALLS_FAILED),
            ("structure_start_refused", Code::STRUCTURE_START_REFUSED),
            ("over_memory", Code::GENERATING_FAILED),
            ("something the helper made up", Code::GENERATING_FAILED),
        ] {
            let Some(Err(error)) = not_ready(&Failed(reason), &keyed, 3, false) else {
                panic!("{reason} isn't a failure")
            };
            assert_eq!(error.code, code, "{reason}");
            if code == Code::GENERATING_FAILED {
                assert_eq!(error.next, "Ask again in a minute.");
            } else {
                assert!(matches!(code.next(), Next::Says(words) if words == error.next), "{reason}: {}", error.next);
            }
        }
    }

    /// Failure: a refusal or failure the helper names says the same as one it doesn't, so the pointer on the note can't say a protected file is never read, or that the helper's missing; or an unknown code says nothing.
    #[test]
    fn each_known_failure_has_its_own_words() {
        let fallback = failure_words("something new");
        assert!(!fallback.is_empty());
        let known = [
            "needs_password",
            "couldnt_read",
            "over_memory",
            "out_of_time",
            "helper_missing",
            "structure_not_sandboxed",
            "structure_inside_another_sandbox",
            "structure_walls_failed",
            "structure_start_refused",
        ];
        for code in known {
            assert_ne!(failure_words(code), fallback, "{code}");
        }
        let distinct: std::collections::HashSet<_> = known.iter().map(|code| failure_words(code)).collect();
        assert_eq!(distinct.len(), known.len(), "each says its own");
        assert_eq!(
            failure_words("no_pages"),
            failure_words("couldnt_read"),
            "a file with no pages is one distillPDF couldn't read"
        );
    }
}
