//! The sidebar's side of generated contents: what a tap on the note does, and how a tab comes to show them. Everything that looks at a file happens on a one-shot thread, never on the session thread or the UI's, since a file on a dead volume can hold a look for as long as it likes. What comes back goes through the session thread's queue.

use crate::{CoreCall, Inner};
use recto_agent::generated::{self, GenState, Generating};
use recto_control::generating::Generator;
use recto_index::corpus::CorpusKey;
use recto_session::{ContentsAction, GeneratedView, TabId};
use std::path::Path;
use std::sync::Arc;

/// A tap on the note over a tab's contents.
pub(crate) fn act(inner: &Inner, id: TabId, action: ContentsAction) {
    let session = &inner.host.session;
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
            look(inner, id, true);
        }
    }
}

/// Generated contents were made somewhere, or failed: each tab waiting for some looks at where its own stand.
pub(crate) fn look_again(inner: &Inner) {
    let waiting = inner.host.session.with_session(|session| session.generated_making());
    for id in waiting {
        look(inner, id, false);
    }
}

/// A tab that's shown generated contents before, across a restart or after they'd left memory, finds them again. Only the active tab, once: what it finds is noted, so this costs a comparison at every other turn.
pub(crate) fn look_if_chosen(inner: &Inner) {
    let wanted = inner.host.session.with_session(|session| {
        let id = session.active()?;
        let chosen = session.tab(id)?.generated_contents && session.generated_view(id).is_none() && session.contents_verdict(id).is_some();
        if chosen {
            session.set_generated_view(id, GeneratedView::NotKept);
        }
        chosen.then_some(id)
    });
    if let Some(id) = wanted {
        look(inner, id, false);
    }
}

/// Looks at where one tab's generated contents stand, on a thread of its own, and asks for them to be made if `ask`.
fn look(inner: &Inner, id: TabId, ask: bool) {
    let Some(generator) = inner.host.session.generator().cloned() else {
        inner.core.push(CoreCall::GeneratedView(
            id,
            GeneratedView::Failed("Generated contents aren't available here."),
            None,
        ));
        return;
    };
    // Which version of the file this look is about: an answer that comes back after the file changed is about the old one, and is dropped.
    let Some((path, pages, version)) = inner
        .host
        .session
        .with_session(|session| Some((session.path_of(id)?, session.tab(id)?.page_count, session.own_stamp(id))))
    else {
        return;
    };
    let core = Arc::clone(&inner.core);
    let spawned = std::thread::Builder::new()
        .name("recto-contents-look".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            if let Some(view) = where_they_stand(&generator, &path, pages, ask) {
                core.push(CoreCall::GeneratedView(id, view, version));
            }
        });
    if spawned.is_err() {
        inner.core.push(CoreCall::GeneratedView(
            id,
            GeneratedView::Failed("They couldn't be asked for just now. Try again."),
            None,
        ));
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
