//! **What a feature's answer to a person's command may read of the app and do in it** ([`super::person`]'s list of answers): the argument every answer is
//! given, in place of the app, on the thread the command ran on. Its fields are private, so what an answer may do is the list of methods below, each named
//! for the need; an answer that needs something else adds one method here, named for that need, which the listing in `headless/inner_guards.rs` shows to a
//! reviewer (docs/FEATURES.md, step 6). Of its 12 methods (2026-11-14), `say` is shared by four answers and the other ten serve one feature each (`print_answered` printing's); a 13th, `image_looked`,
//! an image picked with a click's (2026-11-16).
//! **[`AnswerParts::find_handle`] is the one wide reach:** Find's own, it hands the answer a `FindHandle` whose fields are `pub` (the session, the panes,
//! the page view, the listener), so any answer that calls it can reach those; only Find's does, and narrowing it to what Find's answer needs is in PLAN.
//! What starts on a thread of its own as the person is [`AnswerParts::person`]'s.
//!
//! Built once for each command answered ([`AnswerParts::of`]), from references the app already holds: nothing is copied, locked or allocated to make it.

use super::person::Person;
use super::saver::Saver;
use super::window::Notices;
use super::{Inner, Tools, lock};
use crate::Listener;
use crate::find::FindHandle;
use crate::panes::Panes;
use crate::passwords;
use crate::queue::CoreQueue;
use recto_agent::DrawableLoader;
use recto_control::host::SessionHost;
use recto_reader::Engine;
use recto_update::updater::Updater;
use std::path::Path;
use std::sync::Arc;

/// **What a feature's answer may read of the app and do in it,** for one person's command that ran. Shared references only.
#[derive(Clone, Copy)]
pub struct AnswerParts<'a> {
    notices: &'a Notices,
    core: &'a CoreQueue,
    saver: &'a Saver,
    tools: &'a Tools,
    session: &'a SessionHost<DrawableLoader>,
    panes: &'a Panes,
    engine: &'a Engine,
    listener: &'a (dyn Listener + 'static),
    updates: &'a Updater,
    person: Person<'a>,
}

impl<'a> AnswerParts<'a> {
    /// The answers' parts of `app`.
    pub(in crate::core) fn of(app: &'a Arc<Inner>) -> Self {
        Self {
            notices: app.window.notices(),
            core: &app.core,
            saver: &app.saver,
            tools: &app.tools,
            session: &app.host.session,
            panes: &app.panes,
            engine: &app.engine,
            listener: &*app.listener,
            updates: &app.updates,
            person: Person::of(app),
        }
    }

    /// A line by the page in the person's words, which goes by itself, and the window published again to show it ([`super::person::say`]).
    pub fn say(&self, words: String) {
        super::person::say(self.notices, self.core, words);
    }

    /// A line by the page in the person's words, shown when the window is next published.
    pub fn notice(&self, words: String) {
        self.notices.say(words);
    }

    /// The line by the page taken away, if there's one.
    pub fn take_notice(&self) {
        self.notices.take_notice();
    }

    /// Why a file the person opened didn't open, under the field, in their words.
    pub fn fail_open(&self, words: String) {
        self.notices.fail_open(words);
    }

    /// The record written soon, rather than when it's next due.
    pub fn write_soon(&self) {
        self.saver.write_soon();
    }

    /// The system's sheet asks the person for the password of the file at `path`.
    pub fn ask_password(&self, path: &Path) {
        lock(&self.tools.prompts().passwords).ask(passwords::Target::File, path);
    }

    /// Find's own: what Find reaches of the app, for one call ([`super::find::handle_with`]). Wide: the handle's fields are `pub`, the session among them.
    pub fn find_handle<R>(&self, step: impl FnOnce(&FindHandle<'_, dyn Listener>) -> R) -> R {
        super::find::handle_with(self.session, self.tools, self.core, self.panes, self.engine, self.listener, step)
    }

    /// Whether the person's update command is the menu's.
    pub fn update_menu_marked(&self) -> bool {
        self.updates.menu().marked()
    }

    /// The person's update command no longer the menu's.
    pub fn unmark_update_menu(&self) {
        self.updates.menu().unmark();
    }

    /// Printing's own: what came of the person's print, said to its state, then the window published again and the host told, so the host takes a file
    /// that's ready ([`super::printing::answered`]).
    pub fn print_answered(&self, said: impl FnOnce(&super::printing::Printing)) {
        said(self.tools.printing());
        self.core.push(super::CoreCall::Reconcile);
        self.listener.model_changed();
    }

    /// The snapshot tool's own: what came of the person's snapshot, said to the tool's state, then the window published again, and the host told when
    /// something's ready for the clipboard ([`super::snapshot_tool::answered`]).
    pub fn snapshot_answered(&self, said: impl FnOnce(&super::snapshot_tool::Tool) -> bool) {
        // The host told first, then the window published again, as the tool's own thread did.
        if said(self.tools.tool()) {
            self.listener.copy_ready();
        }
        self.core.push(super::CoreCall::Reconcile);
    }

    /// An image picked with a click's own: what came of a click's look, to the feature's state and the page views that may be waiting for it
    /// ([`super::image_select::answered`]).
    pub fn image_looked<R>(&self, said: impl FnOnce(&super::image_select::ImageSelect, &[Engine]) -> R) -> R {
        said(self.tools.image_select(), &self.panes.engines(self.engine))
    }

    /// What the person may start on a thread of its own.
    pub fn person(&self) -> Person<'a> {
        self.person
    }
}
