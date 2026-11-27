//! **What a feature's menu rows may read of the app and do in it** ([`super::menu_rows`]): the argument every row is given, in place of the app. Its fields
//! are private, so what a row may do is the list of methods below, each named for the need; a row that needs something else adds one method here, named
//! for that need, which the listing in `headless/inner_guards.rs` shows to a reviewer (docs/FEATURES.md, step 6). Of its 21 methods (2026-11-16), 16 serve
//! one feature each (reading_tab viewing's; selected highlights'; copy_selection, select_all and toggle_panel the window's own rows'; can_save saving's;
//! update_installs and mark_update_menu the updates'; zoom_for_person the zoom's; open opening's; open_palette the palette's; toggle_snapshot_tool,
//! arm_snapshot_once and cancel_snapshot_once the snapshot tool's; cancel_print and print_anyway printing's; let_go_image, a 22nd, an image picked
//! with a click's, 2026-11-16), four are shared (published, tell_host, reconcile, person), and one makes it. What starts on a thread of its own as the
//! person is [`RowParts::person`]'s, which a way marked `Agents::AsTheMenu` never calls: an agent's palette run executes that way.
//!
//! Built once for each menu id asked about ([`RowParts::of`]), from references the app already holds: nothing is copied, locked or allocated to make it.

use super::person::Person;
use super::{Inner, Lifecycle, Published, Serving, Tools, Window, lock};
use crate::Listener;
use crate::panes::Panes;
use crate::queue::{CoreCall, CoreQueue};
use recto_reader::Engine;
use recto_reader::tiles::Zoom;
use recto_session::TabId;
use recto_update::updater::Updater;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// **What a feature's menu rows may read of the app and do in it,** for one menu id asked about. Shared references only; a row's way copies it into its
/// closure.
#[derive(Clone, Copy)]
pub struct RowParts<'a> {
    published: &'a Mutex<Published>,
    panes: &'a Panes,
    engine: &'a Engine,
    tools: &'a Tools,
    window: &'a Window,
    life: &'a Lifecycle,
    core: &'a CoreQueue,
    listener: &'a dyn Listener,
    updates: &'a Updater,
    host: &'a super::AppHost,
    serving: &'a Serving,
    person: Person<'a>,
}

impl<'a> RowParts<'a> {
    /// The rows' parts of `app`.
    pub(in crate::core) fn of(app: &'a Arc<Inner>) -> Self {
        Self {
            published: &app.published,
            panes: &app.panes,
            engine: &app.engine,
            tools: &app.tools,
            window: &app.window,
            life: &app.life,
            core: &app.core,
            listener: &*app.listener,
            updates: &app.updates,
            host: &app.host,
            serving: &app.serving,
            person: Person::of(app),
        }
    }

    /// What the session thread last published, read under its lock: never the session's.
    pub fn published<R>(&self, read: impl FnOnce(&Published) -> R) -> R {
        read(&lock(self.published))
    }

    /// The tab in the pane being read, if it has one.
    pub fn reading_tab(&self) -> Option<TabId> {
        self.panes.tab_of(self.panes.reading())
    }

    /// The person's selection in the pane being read: its tab, and its range if it has one.
    pub fn selected(&self) -> Option<(u32, Option<recto_index::select::Range>)> {
        self.panes.reading_engine(self.engine).selection().hot()
    }

    /// The person's selection in the pane being read, to the system's clipboard, by the page view's own copy; `anyway` once they've said to copy a
    /// protected document's text.
    pub fn copy_selection(&self, anyway: bool) {
        // A plain Copy with an image picked is the image's ([`super::image_select::copy`]).
        if super::image_select::copy(self.person, anyway) {
            return;
        }
        // Asked only for a selection that has some text to copy: nothing selected takes no place, and so drops nothing in flight.
        if self.selected().is_some_and(|(_, range)| range.is_some_and(|range| range.start != range.end)) {
            self.tools.tool().text_asked();
        }
        self.panes.reading_engine(self.engine).selection().copy(anyway);
    }

    /// An image picked with a click's own: let go in every pane, its box taken off ([`super::image_select::let_go`]).
    pub fn let_go_image(&self) {
        super::image_select::let_go(self.panes, self.engine);
    }

    /// Everything selected in the pane being read.
    pub fn select_all(&self) {
        self.panes.reading_engine(self.engine).selection().select_all();
    }

    /// Whether Save can be chosen now, as the menu says ([`super::saving::can_save`]).
    pub fn can_save(&self) -> bool {
        super::saving::can_save(self.published, self.life, self.core.lane())
    }

    /// Whether an edit of `tab` the person asked for is still landing ([`crate::queue::lane::Lane::landing`]): a look at the lane, not the session.
    pub fn edit_landing(&self, tab: TabId) -> bool {
        self.core.lane().landing(tab.0)
    }

    /// Whether the updates' menu item installs, rather than checks.
    pub fn update_installs(&self) -> bool {
        self.updates.menu_installs()
    }

    /// Marks the person's next update command as the menu's, unless one is marked already: whether it marked it.
    pub fn mark_update_menu(&self) -> bool {
        self.updates.menu().mark()
    }

    /// The zoom's own: the person's `view --zoom`, run here and now on the thread that asked ([`super::zoom::for_person`]).
    pub fn zoom_for_person(&self, zoom: Zoom) -> bool {
        super::zoom::for_person(self.host, self.serving, zoom)
    }

    /// The person asks for Copy Link now: its place among what's asked of the clipboard, kept for its address when it lands ([`super::snapshot_tool::Tool::link_asked`]).
    pub fn link_asked(&self) {
        self.tools.tool().link_asked();
    }

    /// The person's Copy Link wasn't taken (past a hand-off's last look): its place is given back ([`super::snapshot_tool::Tool::link_not_taken`]).
    pub fn link_not_taken(&self) {
        self.tools.tool().link_not_taken();
    }

    /// The tools panel open or shut.
    pub fn toggle_panel(&self) {
        self.window.toggle_panel();
    }

    /// The host told its model changed, to draw it again.
    pub fn tell_host(&self) {
        self.listener.model_changed();
    }

    /// The session thread asked to publish again.
    pub fn reconcile(&self) {
        self.core.push(CoreCall::Reconcile);
    }

    /// A file the person opens ([`super::opening::open_for_person`]).
    pub fn open(&self, path: PathBuf) {
        super::opening::open_for_person(self.life, self.core, path, None);
    }

    /// ⌘K's palette opened.
    pub fn open_palette(&self) {
        self.tools.palette().open();
    }

    /// The snapshot tool's own: the pointer's tool on or off. Whether it's on now.
    pub fn toggle_snapshot_tool(&self) -> bool {
        self.tools.tool().toggle()
    }

    /// The snapshot tool's own: Capture, the tool on for one capture. Whether anything changed.
    pub fn arm_snapshot_once(&self) -> bool {
        self.tools.tool().arm_once()
    }

    /// The snapshot tool's own: Escape before a capture is taken. Whether it ended one.
    pub fn cancel_snapshot_once(&self) -> bool {
        self.tools.tool().cancel_once()
    }

    /// Printing's own: the person's print being prepared stopped.
    pub fn cancel_print(&self) {
        self.tools.printing().cancel();
    }

    /// Printing's own: what a notice's Print anyway asks again as the person's `print --anyway`, once, if that notice is still the one refused and no
    /// other print is asked for ([`super::printing::Printing::anyway`]). A command not taken (a hand-off froze the app since the menu looked) leaves the
    /// print asked for not running, so it isn't left counted as under way.
    pub fn print_anyway(&self, serial: u64) {
        if let Some(command) = self.tools.printing().anyway(serial) {
            self.tools.printing().taken(self.person.submit(command));
        }
    }

    /// What the person may start on a thread of its own.
    pub fn person(&self) -> Person<'a> {
        self.person
    }
}
