//! The sidebar's side of generated contents: what a tap on the note does, and how a tab comes to show them. Everything that looks at a file happens on a one-shot thread, never on the session thread or the UI's, since a file on a dead volume can hold a look for as long as it likes. What comes back goes through the session thread's queue.
//!
//! It's given the session host and a [`Push`]: the one way an answer gets back to the session thread, which `recto-ffi` makes over its queue. A push is cloned only for a look's
//! thread, which owns its copy.

use recto_agent::DrawableLoader;
use recto_agent::generated::{self, GenState, Generating};
use recto_control::generating::Generator;
use recto_control::host::SessionHost;
use recto_index::corpus::CorpusKey;
use recto_platform::FileStamp;
use recto_session::{ContentsAction, GeneratedView, TabId};
use std::path::Path;
use std::sync::Arc;

/// Hands a tab's generated contents view to the session thread, with the version of the file it's about (`None`: whichever). Must return at once: it's called on a look's
/// thread and on the session thread.
pub trait Push: Fn(TabId, GeneratedView, Option<FileStamp>) + Clone + Send + 'static {}

impl<P: Fn(TabId, GeneratedView, Option<FileStamp>) + Clone + Send + 'static> Push for P {}

/// A tap on the note over a tab's contents.
pub fn act(session: &SessionHost<DrawableLoader>, push: &impl Push, id: TabId, action: ContentsAction) {
    match action {
        ContentsAction::ShowOwn => session.with_session(|session| session.choose_generated(id, false)),
        ContentsAction::ShowGenerated => session.with_session(|session| session.choose_generated(id, true)),
        ContentsAction::Stop => {
            // Stopped by the reader: the helper goes, and the offer's back. Nothing's read for it.
            if let (Some(generator), Some(path)) = (session.generator(), session.with_session(|session| session.path_of(id))) {
                generator.cancel_path(&path);
            }
            session.with_session(|session| {
                session.choose_generated(id, false);
                session.forget_generated(id);
            });
        }
        ContentsAction::Generate => {
            session.with_session(|session| {
                session.choose_generated(id, true);
                session.set_generated_view(id, GeneratedView::Making);
            });
            look(session, push, id, true);
        }
    }
}

/// Generated contents were made somewhere, or failed: each tab waiting for some looks at where its own stand.
pub fn look_again(session: &SessionHost<DrawableLoader>, push: &impl Push) {
    let waiting = session.with_session(|session| session.generated_making());
    for id in waiting {
        look(session, push, id, false);
    }
}

/// A tab that's shown generated contents before, across a restart or after they'd left memory, finds them again. Only the active tab, once: what it finds is noted, so this costs a comparison at every other turn.
pub fn look_if_chosen(session: &SessionHost<DrawableLoader>, push: &impl Push) {
    let wanted = session.with_session(|session| {
        let id = session.active()?;
        let chosen = session.tab(id)?.generated_contents && session.generated_view(id).is_none() && session.contents_verdict(id).is_some();
        if chosen {
            session.set_generated_view(id, GeneratedView::NotKept);
        }
        chosen.then_some(id)
    });
    if let Some(id) = wanted {
        look(session, push, id, false);
    }
}

/// Looks at where one tab's generated contents stand, on a thread of its own, and asks for them to be made if `ask`.
fn look(session: &SessionHost<DrawableLoader>, push: &impl Push, id: TabId, ask: bool) {
    let Some(generator) = session.generator().cloned() else {
        push(id, GeneratedView::Failed("Generated contents aren't available here."), None);
        return;
    };
    // Which version of the file this look is about: an answer that comes back after the file changed is about the old one, and is dropped.
    let Some((path, pages, version)) = session.with_session(|session| Some((session.path_of(id)?, session.tab(id)?.page_count, session.own_stamp(id)))) else {
        return;
    };
    let answer = push.clone();
    let spawned = std::thread::Builder::new()
        .name("recto-contents-look".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            if let Some(view) = where_they_stand(&generator, &path, pages, ask) {
                answer(id, view, version);
            }
        });
    if spawned.is_err() {
        push(id, GeneratedView::Failed("They couldn't be asked for just now. Try again."), None);
    }
}

/// What the sidebar should show for a file's generated contents now, or `None` if it's still what it was.
fn where_they_stand(generator: &Generator, path: &Path, pages: u32, ask: bool) -> Option<GeneratedView> {
    let Ok(key) = CorpusKey::of_file(path) else {
        return Some(GeneratedView::Failed("The file can't be looked at just now."));
    };
    if let Some(made) = generated::find(generator, path, key) {
        // A scan with no text layer, or a document set all in one size, has nothing to list: said, rather than shown as an empty list.
        if made.entries.is_empty() {
            return Some(GeneratedView::NoneFound);
        }
        return Some(GeneratedView::Ready(Arc::new(made.toc())));
    }
    if ask {
        generator.want(path, key, pages);
    }
    match generator.state(key) {
        GenState::Queued | GenState::Running => ask.then_some(GeneratedView::Making),
        GenState::Refused(code) | GenState::Failed(code) => Some(GeneratedView::Failed(failure_words(&code))),
        GenState::Idle => Some(if ask { GeneratedView::Making } else { GeneratedView::NotKept }),
    }
}

/// Why, for the pointer resting on the note. Rust's words.
fn failure_words(code: &str) -> &'static str {
    match code {
        "needs_password" => "This file needs a password, and nothing of a protected file is read for this.",
        "couldnt_read" | "no_pages" => "distillPDF couldn't read this file.",
        "over_memory" => "Finding the headings took more memory than it's allowed.",
        "out_of_time" => "Finding the headings took longer than it's allowed.",
        "helper_missing" => "The helper that finds headings isn't installed.",
        _ => "Something went wrong finding the headings. Click to try again.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Failure: a refusal or failure the helper names says the same as one it doesn't, so the pointer on the note can't say a protected file is never read, or that the helper's missing; or an unknown code says nothing.
    #[test]
    fn each_known_failure_has_its_own_words() {
        let fallback = failure_words("something new");
        assert!(!fallback.is_empty());
        let known = ["needs_password", "couldnt_read", "over_memory", "out_of_time", "helper_missing"];
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

    /// Failure: with no helper (a session with no generator), asking for generated contents leaves the note spinning on Making for good, or the answer carries a file version and is dropped as stale; or it starts a thread to say so.
    #[test]
    fn with_no_helper_the_note_says_so_at_once() {
        let session = SessionHost::default();
        let pushed = Arc::new(Mutex::new(Vec::new()));
        let got = Arc::clone(&pushed);
        let push = move |id: TabId, view: GeneratedView, version: Option<FileStamp>| crate::lock(&got).push((id, view, version));
        act(&session, &push, TabId(7), ContentsAction::Generate);
        let pushed = crate::lock(&pushed);
        assert_eq!(pushed.len(), 1, "{pushed:?}");
        assert_eq!(pushed[0].0, TabId(7));
        assert!(matches!(pushed[0].1, GeneratedView::Failed(_)), "{:?}", pushed[0].1);
        assert!(pushed[0].2.is_none(), "about whichever version");
    }

    /// Failure: looking again when nothing's waiting asks the helper or pushes something anyway, on every Finished the generator sends.
    #[test]
    fn looking_again_with_nothing_waiting_does_nothing() {
        let session = SessionHost::default();
        let pushed = Arc::new(Mutex::new(0_usize));
        let got = Arc::clone(&pushed);
        let push = move |_: TabId, _: GeneratedView, _: Option<FileStamp>| *crate::lock(&got) += 1;
        look_again(&session, &push);
        look_if_chosen(&session, &push);
        assert_eq!(*crate::lock(&pushed), 0);
    }
}
