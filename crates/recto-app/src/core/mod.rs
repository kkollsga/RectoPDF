//! **The app's machinery, as the window and the socket share it:** the app's state and its parts, the session thread and what it publishes, the host every command
//! runs on, and the window's features with state of their own, a file per concern. The bridge forwards to it.

pub mod actions;
pub mod find;
pub mod generated_contents;
pub mod host;
pub mod palette;
pub mod printing;
pub mod qa;
pub mod reconcile;
pub mod saver;
pub mod saving;
pub mod session;
pub mod sleeper;
pub mod snapshot_tool;
pub mod start;
pub mod updates;

mod closing;
mod comparison;
mod contents;
pub mod controls;
mod editing;
#[cfg(test)]
mod headless;
mod launching;
pub mod menu_rows;
mod navigation;
mod opening;
pub mod person;
pub mod places;
mod quitting;
mod record_commands;
pub mod showing;
mod unlocking;
mod viewing;
pub mod window;
mod window_rows;
pub mod zoom;

pub use host::*;
use opening::*;
pub use reconcile::*;
use recto_agent::Code;
pub use session::*;
pub use start::*;
use unlocking::*;
pub use window::{AppState, Window};

use crate::RecentFile;
use crate::SelectionNotice;
use crate::SidebarRow;
use crate::hand_off::{BusyItem, HandOffCheck, HandOffError, HandOffReport};
use crate::menus::{self, HostAction};
use crate::queue::{CoreCall, CoreQueue, FileRead, Flushed, Reading};
use crate::reader_view::ReaderView;
use crate::records::{HighlightRect, ReaderOp, ScreenInfo};
use crate::watchdog::Watchdog;
use crate::{ContentsEntry, PaneSnapshot, panes};
use crate::{linking, lock, passwords};
use qa::QaBridge;
use recto_agent::{AgentError, Command, Condition, ConditionState, DrawableLoader, Event};
use recto_control::StatePaths;
use recto_control::host::{Host, SessionHost, kept_json, store_error};
use recto_control::indexing::Notifier;
use recto_control::record_store::{Loaded, notice_for};
use recto_control::server::{Announcer, Limits, Server, Stopper};
use recto_core::{DocFacts, OpenError};
use recto_platform::FileStamp;
use recto_reader::engine::EngineEvents;
use recto_reader::tiles::PageMap;
use recto_reader::{Engine, PaperKind};
use recto_session::HistorySource;
use recto_session::record::{self, FileStatus, look_at_file};
use recto_session::{DocLoader, DrawNote, Paper, TabId, ViewSettings};
use saver::Saver;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where the chosen paper is remembered, in the state folder.
pub const PAPER_FILE: &str = "paper";

/// Whether the agent icon is hidden, there too. It's here only if it is.
pub const AGENT_ICON_FILE: &str = "agent-icon-hidden";

/// How long the window waits for the state to be read. Longer means the state's disk isn't answering, and the app starts without it for now.
pub const RESTORE_PATIENCE: Duration = Duration::from_secs(1);

/// How long the window waits for the page to be drawn before it shows anyway. About what a heavy page takes.
pub const SHOW_PATIENCE: Duration = Duration::from_millis(1200);

/// The thread a first frame's news goes on: room to start a worker and draw, and only address space until it's used.
pub const UPDATE_FRAME_STACK: usize = 1024 * 1024;

/// How long a socket that's taken is tried again, in case what has it is on its way out.
pub const SOCKET_PATIENCE: Duration = Duration::from_secs(1);

/// How long a quit waits for the state to go out, before asking.
pub const QUIT_PATIENCE: Duration = Duration::from_millis(crate::listener::QUIT_PATIENCE_MS);

/// The most contents rows one call hands over. More than any screen shows.
pub const MAX_CONTENTS_FETCH: u32 = 256;

/// **`qa save-from`'s signature check,** on what was found of the file's signatures: a person's signature refuses it, and so does a file that couldn't be looked
/// at, as the save itself refuses what it can't look through (review round 2).
pub fn save_from_signature_check(facts: Result<recto_core::save::SignatureFacts, recto_core::OpenError>) -> Result<(), AgentError> {
    match facts {
        Ok(facts) if !facts.has_persons_signature() => Ok(()),
        Ok(_) => Err(AgentError::new(
            Code::SIGNED_NEEDS_COPY,
            recto_core::save::SaveError::SignedNeedsCopy.message(),
            recto_agent::exit::FAILED,
        )),
        Err(error) => Err(AgentError::new(
            Code::lower(error.code()),
            format!("{} Its signatures couldn't be looked at, so nothing was put in place.", error.message()),
            recto_agent::exit::FAILED,
        )),
    }
}

/// What the session thread remembers between looks.
#[derive(Default)]
pub struct Synced {
    /// Each pane, left to right: one, or a comparison's.
    pub panes: Vec<PaneSynced>,
    pub paper: Option<PaperKind>,
    /// The tab being read, whose contents the sidebar has.
    pub active: Option<TabId>,
    /// How many page views there are.
    pub count: usize,
}

/// The state the UI thread pulls, swapped whole. A short lock, never held across anything slow.
pub struct Published {
    /// Files opened lately, newest first, for Open Recent and ⌘K, so neither asks the session.
    pub recent: Arc<Vec<RecentFile>>,
    /// What `recent` was built from: the session's list's version and the tabs' version.
    pub recent_key: Option<(u64, u64)>,
    /// Each pane's contents and page labels, for a link's hint: the UI thread reads them here and never asks the session.
    pub pane_tocs: Vec<Option<Arc<recto_core::Toc>>>,
    pub sidebar: Arc<crate::SidebarView>,
    pub tabs: Arc<Vec<crate::SidebarRow>>,
    pub contents: Arc<Vec<ContentsEntry>>,
    pub rail_filled: u32,
    pub reader: ReaderView,
}

impl Published {
    /// What's published before the session thread's first pass: no tabs, the home screen, white paper.
    pub fn empty() -> Self {
        Self {
            recent: Arc::new(Vec::new()),
            recent_key: None,
            pane_tocs: Vec::new(),
            contents: Arc::new(Vec::new()),
            rail_filled: 0,
            tabs: Arc::new(Vec::new()),
            sidebar: Arc::new(crate::empty_sidebar()),
            reader: ReaderView::home(),
        }
    }
}

pub struct Inner {
    /// Small state, and no thread or timer of its own at rest.
    pub updates: Arc<recto_update::updater::Updater>,
    pub host: Arc<AppHost>,
    pub engine: Engine,
    /// A comparison's panes past the first, and what the UI thread needs to keep them in step.
    pub panes: panes::Panes,
    pub core: Arc<CoreQueue>,
    pub(crate) listener: Arc<dyn crate::Listener>,
    pub published: Mutex<Published>,
    pub saver: Arc<Saver>,
    pub(crate) sleeper: sleeper::Sleeper,
    pub qa: Option<Arc<QaBridge>>,
    /// Passwords typed for protected files that are open, shared with every page view's workers. Memory only.
    pub(crate) keys: Arc<recto_reader::engine::Keyring>,
    /// Shared with the control socket's thread, which holds nothing else of the core while it serves, so the session's end goes by the machine too.
    pub life: Arc<Lifecycle>,
    pub serving: Serving,
    pub window: Window,
    pub tools: Tools,
}

/// **Where the app is in its life:** launch, the state's coming in, the window going up, the person's last touch, and a hand-off with what it waits for. What step
/// R1.4 makes a state machine on the clock.
pub struct Lifecycle {
    /// What the host knows of this copy, for the thread that reads the state.
    pub(crate) update_facts: Mutex<Option<recto_update::updater::Facts>>,
    /// What the bridge's own readings go by: the first frame's time, its show patience and the contents' follow. The core's go by [`Self::clock`].
    pub started: Instant,
    /// The host's clock, a test's fake one: what the idle time and the window's patience for input go by (step R1.4).
    pub(crate) clock: Arc<dyn crate::clock::Clock>,
    /// **The state's coming in, the window's patience for it, what's shown and a hand-off's freeze, as one state machine** ([`crate::lifecycle::Life`]). The lock is
    /// held only inside [`Self::step`] and the queries beside it, for a copy or a transition of a few bytes: never across carrying out an `Out`, a listener call, a
    /// queue push, another lock or any I/O. So it's taken on the UI thread too, bounded. The time since input isn't in it: what the UI thread sets on every event
    /// is an atomic store ([`Self::last_input_ms`]), passed to what asks as a value.
    life: Mutex<crate::lifecycle::Life>,
    /// Reading the file a restored active tab is on. Nothing's drawn until it's looked at, so the wrong thing can't show.
    pub holding_tab: Mutex<Option<TabId>>,
    /// In a QA run, files being read wait here, as they would for a slow disk.
    pub(crate) qa_hold_reads: AtomicBool,
    /// Files being read now, which a hand-off waits for.
    pub(crate) reads_running: AtomicU32,
    /// The one pass over the other tabs' files after a restore, while it's under way: what a test waits for, as nothing else does.
    pub(crate) others_looking: AtomicU32,
    /// A person's commands on threads of their own (`person::submit`), to the end of what came of them: what a test waits for. The host counts them for a
    /// hand-off as it counts any command.
    pub(crate) persons_running: AtomicU32,
    /// Sheets and panels over the window now, which a hand-off waits for.
    pub sheets: AtomicU32,
    /// When the user last touched the app, in milliseconds of `clock`. One number, set by what the event monitor calls, with no timer.
    pub(crate) last_input_ms: AtomicU64,
    /// What the window's first showing had on it.
    pub first_shown: Mutex<Option<Value>>,
    /// When the window was last put up, for how long the patience had run when it was shown.
    pub window_opened_at: Mutex<Option<Instant>>,
    /// Whether the last yes `ready_to_show` gave was the cap's, for the record to say so.
    pub shown_by_cap: AtomicBool,
}

/// **The control socket:** where the state lives, the server waiting for the session to be as it was, and what stops it and announces to its watchers.
pub struct Serving {
    /// The control socket, ready to run once the session's as it was, so nothing asks about an empty one or opens over it.
    pub(crate) waiting_server: Mutex<Option<Server>>,
    pub(crate) stopper: Mutex<Option<Stopper>>,
    pub(crate) announcer: Mutex<Option<Announcer>>,
    pub paths: Option<StatePaths>,
    /// Whether the socket is served at all. The app's always is; a test's core has none unless it asks, so nothing binds it later either.
    pub(crate) socket: bool,
    /// What the session thread announced, and the person's commands sent, for a test that's asked to hear them
    /// ([`crate::testkit::headless::HeadlessApp::hear_events`]): a test's core has no socket, and its watchers are who hear a person's events. Nothing's kept
    /// until a test asks.
    #[cfg(any(test, feature = "testkit"))]
    pub(crate) heard: Mutex<crate::testkit::heard::Overheard>,
    /// Where a test comes between the reads of the views' places and their take-in, and the looks counted: [`crate::testkit::place_seams::PlaceSeams`].
    #[cfg(any(test, feature = "testkit"))]
    pub(crate) place_seams: crate::testkit::place_seams::PlaceSeams,
}

/// **The window's features with state of their own:** the sheets that ask, saving's, the snapshot tool, the sidebar's folds, the chrome's marks, printing, ⌘K and
/// Find's field.
pub struct Tools {
    /// The sheets that ask: a password, or whether to open a link.
    pub(crate) prompts: crate::prompts::Prompts,
    /// Saves from the window: how many run, and what one that failed says.
    pub(crate) save_ui: saving::SaveUi,
    /// The snapshot tool: on or off, the box being drawn, and what's on its way to the clipboard.
    pub tool: snapshot_tool::Tool,
    pub folds: crate::sidebar_access::Folds,
    pub chrome: crate::chrome::Chrome,
    /// A print being prepared, or waiting for the host to hand to the system.
    pub printing: printing::Printing,
    /// ⌘K's palette, on the UI thread.
    pub(crate) palette: crate::palette::Palette,
    /// The Find field as the session thread last set it, for the UI thread.
    pub(crate) find_field: Mutex<crate::find::Field>,
}

// `new` is public for the bridge, which builds the app; launch is no default state.
#[allow(clippy::new_without_default)]
impl Lifecycle {
    /// At launch: nothing read, nothing shown, no hand-off.
    pub fn new(clock: Arc<dyn crate::clock::Clock>) -> Self {
        Self {
            started: Instant::now(),
            clock,
            life: Mutex::new(crate::lifecycle::Life::default()),
            holding_tab: Mutex::new(None),
            first_shown: Mutex::new(None),
            window_opened_at: Mutex::new(None),
            shown_by_cap: AtomicBool::new(false),
            last_input_ms: AtomicU64::new(0),
            update_facts: Mutex::new(None),
            reads_running: AtomicU32::new(0),
            persons_running: AtomicU32::new(0),
            others_looking: AtomicU32::new(0),
            sheets: AtomicU32::new(0),
            qa_hold_reads: AtomicBool::new(false),
        }
    }

    /// Now, by the clock, in its milliseconds.
    pub(crate) fn now_ms(&self) -> u64 {
        self.clock.now_ms()
    }

    /// **What happens to the app's life,** now by the clock. The lock's let go of before this returns: the caller carries out what it comes to.
    #[must_use]
    pub fn step(&self, event: crate::lifecycle::Event) -> Option<crate::lifecycle::Out> {
        let now = self.now_ms();
        lock(&self.life).step(event, now)
    }

    /// The app's life as it stands, a copy.
    fn life(&self) -> crate::lifecycle::Life {
        *lock(&self.life)
    }

    /// Whether the state's been read and taken in.
    pub fn state_read(&self) -> bool {
        self.life().restored()
    }

    /// Whether what the host shows has been made from the state.
    pub(crate) fn shown_with_state(&self) -> bool {
        self.life().shown() == crate::lifecycle::Shown::WithState
    }

    /// Whether the window went up without the state and was made again from it.
    pub(crate) fn remade(&self) -> bool {
        self.life().remade()
    }

    /// Whether the window takes what a person does, now.
    pub(crate) fn takes_input(&self) -> bool {
        let now = self.now_ms();
        self.life().takes_input(now)
    }

    /// How long until the patience for the state is over by the session thread's own deadline, if it waits on one.
    pub(crate) fn due(&self) -> Option<Duration> {
        let now = self.now_ms();
        self.life().due(now)
    }

    /// Whether the host's been told to quit for a hand-off that's written.
    pub(crate) fn told_to_quit(&self) -> bool {
        self.life().hand_off() == crate::lifecycle::HandOff::Quitting
    }
}

impl Tools {
    /// At launch: every tool at rest.
    pub fn new(save_step: &Arc<AtomicU8>) -> Self {
        Self {
            prompts: crate::prompts::Prompts::default(),
            save_ui: saving::SaveUi::new(Arc::clone(save_step)),
            tool: snapshot_tool::Tool::default(),
            folds: crate::sidebar_access::Folds::default(),
            chrome: crate::chrome::Chrome::default(),
            printing: printing::Printing::default(),
            palette: crate::palette::Palette::default(),
            find_field: Mutex::new(crate::find::Field::default()),
        }
    }
}

/// What's in flight, for an update to wait for. A save from the window counts here, and an agent's save or an export is a command.
pub fn hand_off_check_parts(inner_reads: u32, sheets: u32, commands: u32, saves: u32, printing: bool) -> Vec<BusyItem> {
    let item = |code: &str, message: &str| BusyItem {
        code: code.to_string(),
        message: message.to_string(),
    };
    let mut busy = Vec::new();
    if sheets > 0 {
        busy.push(item("sheet_open", "A sheet or panel is open over the window."));
    }
    if commands > 0 {
        busy.push(item("agent_command", "An agent's command is running."));
    }
    if inner_reads > 0 {
        busy.push(item("file_opening", "A file is being opened."));
    }
    // An update waits for a save to finish, then goes on with what it wrote (SPEC decision 24).
    if saves > 0 {
        busy.push(item("saving", "A document is being saved."));
    }
    // A print's pages are drawn by a command-like thread, but once they're ready the system's printing has them, which no thread here counts.
    if printing {
        busy.push(item("printing", "A print is being prepared or printed."));
    }
    busy
}

pub fn hand_off_check(host: &AppHost) -> HandOffCheck {
    let Some(inner) = host.saver.inner() else {
        return HandOffCheck {
            safe: false,
            busy: Vec::new(),
            seconds_since_input: 0.0,
        };
    };
    inner.hand_off_check()
}

impl Inner {
    pub fn hand_off_check(&self) -> HandOffCheck {
        let busy = hand_off_check_parts(
            self.life.reads_running.load(Ordering::SeqCst),
            self.life.sheets.load(Ordering::SeqCst),
            self.host.running.load(Ordering::SeqCst),
            self.tools.save_ui.running.load(Ordering::SeqCst),
            self.tools.printing.busy(),
        );
        let last = self.life.last_input_ms.load(Ordering::Relaxed);
        let now = self.life.now_ms();
        HandOffCheck {
            safe: busy.is_empty(),
            busy,
            seconds_since_input: now.saturating_sub(last) as f64 / 1000.0,
        }
    }

    /// **The hand-off.** If nothing's in flight, stops taking commands and what a person does, writes everything, forces it all to the drive, and reads it back. On success the state is exactly what comes back, and the caller quits with no prompt, by [`AppCore::hand_off_quit_now`], which goes only after this has said yes. Otherwise it says why, everything's taken again, and nothing's changed. Waits on the disk, so never on the UI thread.
    pub fn hand_off(&self, limit: Duration) -> Result<HandOffReport, HandOffError> {
        let check = self.hand_off_check();
        if !check.safe {
            return Err(HandOffError::Busy { items: check.busy });
        }
        self.carry_out(self.life.step(crate::lifecycle::Event::HandOffWriting));
        let undo = || self.carry_out(self.life.step(crate::lifecycle::Event::HandOffUndone));
        // What slipped in as that was set is counted by now, and what's coming will see the flag.
        let check = self.hand_off_check();
        if !check.safe {
            undo();
            return Err(HandOffError::Busy { items: check.busy });
        }
        match self.saver.flush(true, limit) {
            Ok(report) => {
                // A file still being read when that gave up would have shown as an error: this is what says nothing's left over.
                let check = self.hand_off_check();
                if !check.safe {
                    undo();
                    return Err(HandOffError::Busy { items: check.busy });
                }
                self.carry_out(self.life.step(crate::lifecycle::Event::HandOffWritten));
                Ok(report.into())
            }
            Err(error) => {
                undo();
                Err(HandOffError::NotSaved {
                    code: error.code.to_string(),
                    message: error.message,
                })
            }
        }
    }

    /// Whether commands and what a person does are being taken.
    pub fn set_frozen(&self, frozen: bool) {
        self.host.handing_off.store(frozen, Ordering::SeqCst);
        self.core.frozen.store(frozen, Ordering::SeqCst);
    }

    /// Quits now, if a hand-off has really got this far. Says whether the host's been told to: the first ask tells it, and one after says yes and tells it nothing
    /// more ([`crate::lifecycle::Event::HandOffQuit`]).
    pub fn hand_off_quit_now(&self) -> bool {
        self.carry_out(self.life.step(crate::lifecycle::Event::HandOffQuit));
        self.life.told_to_quit()
    }

    /// The reader's moved on from what they were told about the session as a whole.
    pub fn notice_read(&self) {
        if lock(&self.host.notice).take().is_some() {
            self.saver.view_changed();
            self.core.push(CoreCall::Reconcile);
        }
    }

    /// Everything the record holds and QA compares: the record as it would be written, and each tab's edits in full.
    pub fn state_json(&self) -> Value {
        let (record, _) = self.gather_record();
        let mut value = serde_json::to_value(&record).unwrap_or(Value::Null);
        let lists = self.host.session.with_session(|session| {
            session
                .tabs()
                .iter()
                .filter_map(|tab| Some((tab.id.0.to_string(), serde_json::to_value(session.list_record_anywhere(tab.id)?).ok()?)))
                .collect::<serde_json::Map<_, _>>()
        });
        value["edits_full"] = Value::Object(lists);
        value
    }

    /// What QA reads of how a restore and the record went, none of it what the user sees.
    pub fn record_report(&self) -> Value {
        let (writes, appends) = self.saver.store.writes();
        json!({ "first_shown": *lock(&self.life.first_shown), "remade": self.life.remade(), "sleep": self.sleeper.report(), "writes": writes, "appends": appends, "wrote_bytes": self.saver.store.wrote_bytes(), "failed": self.saver.has_failed() })
    }

    /// A sheet or panel opened over the window, or closed. An update waits for it.
    pub fn sheet_changed(&self, open: bool) {
        if open {
            self.life.sheets.fetch_add(1, Ordering::SeqCst);
        } else {
            let _ = self.life.sheets.try_update(Ordering::SeqCst, Ordering::SeqCst, |count| count.checked_sub(1));
        }
    }

    pub fn note_nearby(&self) {
        self.life.last_input_ms.store(self.life.now_ms(), Ordering::Relaxed);
    }

    pub fn note_input(&self) {
        self.life.last_input_ms.store(self.life.now_ms(), Ordering::Relaxed);
        // One lock per input, and an uncontended one. It's what lets a notice go when the reader has moved on.
        self.notice_read();
    }
}

#[cfg(test)]
mod tests {
    use recto_control::record_store::{Flavor, RecordStore};
    use recto_reader::EngineConfig;
    /// Failure (review round 2 of stage 2): `qa save-from` lets a file whose signatures couldn't be looked at through the signature check, so a crash test can put a file in place over a signed one the save itself would refuse. A signed file and one that won't open are both refused; an unsigned one goes.
    #[test]
    fn save_from_refuses_a_file_it_cant_look_at() {
        use recto_core::save::SignatureFacts;
        assert!(super::save_from_signature_check(Ok(SignatureFacts::default())).is_ok());
        assert_eq!(
            super::save_from_signature_check(Ok(SignatureFacts {
                signed: true,
                ..SignatureFacts::default()
            }))
            .unwrap_err()
            .code,
            "signed_needs_copy"
        );
        let unreadable = recto_core::save::signature_facts_of(std::path::Path::new("/nowhere/at/all.pdf"));
        assert!(unreadable.is_err());
        assert_eq!(super::save_from_signature_check(unreadable).unwrap_err().code, "not_found");
    }

    use super::*;

    /// Failure: an update goes ahead while a document is being saved from the window, so the app quits under the save (SPEC decision 24: a running save finishes first). Each thing in flight is named, and a save is one of them.
    #[test]
    fn a_hand_off_waits_for_a_save() {
        assert!(hand_off_check_parts(0, 0, 0, 0, false).is_empty());
        let busy = hand_off_check_parts(0, 0, 0, 1, false);
        assert_eq!(busy.iter().map(|item| item.code.as_str()).collect::<Vec<_>>(), ["saving"]);
        assert_eq!(hand_off_check_parts(1, 1, 1, 1, true).len(), 5);
    }

    /// Failure: an update quits the app mid-print, because the check only counted running commands, and a print's thread is done before the system's printing has its file (review, 2026-11-08).
    #[test]
    fn a_hand_off_waits_for_a_print() {
        let busy = hand_off_check_parts(0, 0, 0, 0, true);
        assert_eq!(busy.iter().map(|item| item.code.as_str()).collect::<Vec<_>>(), ["printing"]);
    }

    struct TestEngineEvents;

    impl EngineEvents for TestEngineEvents {
        fn ops_ready(&self) {}
        fn note_changed(&self) {}
        fn page_moved(&self) {}
        fn page_drawn(&self, _tab: u32, _page: u32, _failure: Option<&'static str>) {}
    }

    /// Failure (review 1 of the slow mode): an arm of the session thread's loop left it early, with `continue`, and skipped what every call ends with. The state's coming in announced nothing it did and left Find's field behind for the window made from it (CI, run 43062872948), and a flush's own changes were never announced. Only these leave early, each for its reason: a person's open before the state's in, put off whole; a drawn page, which announces itself and makes no view; and the state's coming in, after its tail, before the host's told.
    #[test]
    fn every_call_on_the_session_thread_ends_with_what_every_call_ends_with() {
        let source = include_str!("session.rs");
        let body = &source[source.find("fn session_thread(").unwrap()..source.find("fn after_call(").unwrap()];
        let body = &body[body.find("match call {").unwrap()..];
        let mut arm = "";
        let mut early = Vec::new();
        let mut tail_before = std::collections::HashSet::new();
        for line in body.lines() {
            let trimmed = line.trim_start();
            if line.len() - trimmed.len() == 12 && trimmed.starts_with("CoreCall::") {
                arm = trimmed.split(['(', ' ']).next().unwrap();
            }
            if trimmed.starts_with("after_call(") {
                tail_before.insert(arm);
            }
            if trimmed == "continue;" {
                early.push(arm);
            }
        }
        assert_eq!(early, ["CoreCall::Restored", "CoreCall::Run", "CoreCall::Page"], "the arms that leave early");
        assert!(
            tail_before.contains("CoreCall::Restored"),
            "the state's coming in ends with the tail before it goes"
        );
    }

    /// Failure: an agent at Control quits the app out from under the person using it, with no prompt, by `recto stop`. In a normal app it's refused, and the words are for whoever's asking.
    #[test]
    fn stop_is_refused_by_a_normal_app() {
        let state = recto_testpdf::TempDir::new("rf-");
        let host = AppHost {
            engine: Engine::new(EngineConfig::default(), Arc::new(TestEngineEvents)),
            session: SessionHost::new(DrawableLoader),
            core: Arc::new(CoreQueue::default()),
            qa: None,
            saver: Arc::new(Saver::new(
                RecordStore::open(&StatePaths::overridden(&state).unwrap(), Flavor::App),
                Arc::new(CoreQueue::default()),
                Arc::new(crate::clock::SystemClock::new()),
            )),
            running: AtomicU32::new(0),
            handing_off: AtomicBool::new(false),
            notice: Mutex::new(None),
            watchers: Mutex::new(None),
        };
        assert_eq!(host.stop_refusal().expect("refused").code, "only_the_user_quits");
    }
}
