//! **What `Inner`'s moves must not change, pinned before they start** (R3.7; AGENTS.md, "guards go in before a move"): the size of the app's state and of
//! each of its parts, and what each lets other code reach. What every control sends and the host is told is held by the coverage test's golden
//! (`coverage.rs`); what a person's action allocates on the window's thread, by `tests/inner_allocations.rs`, in a binary of its own for its allocator.

use crate::core::{AppState, Inner, Lifecycle, Published, Serving, Tools, WindowParts};
use std::mem::size_of;

/// Failure: the app's state or one of its parts grows (a field, a lock or a buffer added, a part given its own `Arc` of the queue), which nothing else sees,
/// as there's one of each in the app. Each is held at no more than its budget, in this test build (whose `Serving` has two fields more than the app's).
///
/// **What each of the two guards is for.** A field another part of the code can reach, and its type, is the listing's below: a field added, or a `Mutex`
/// where an atomic was, changes a line there, named (each seen failing on a seeded change, 2026-11-13). This one holds what the listing can't see: a private
/// field, and growth inside a field's own type from another module or crate. It's a budget and not an exact size, so a merge that shrinks a part (or a
/// type of another crate a part holds) passes; a budget only comes down, by hand, and goes up only in a commit that says why.
#[test]
fn the_apps_state_and_its_parts_stay_within_their_budgets() {
    let sizes = [
        ("Inner", size_of::<Inner>()),
        ("Lifecycle", size_of::<Lifecycle>()),
        ("Serving", size_of::<Serving>()),
        ("WindowParts", size_of::<WindowParts>()),
        ("Tools", size_of::<Tools>()),
        ("Published", size_of::<Published>()),
        ("AppState", size_of::<AppState>()),
    ];
    let names: Vec<&str> = sizes.iter().map(|(name, _)| *name).collect();
    assert_eq!(names, BUDGETS.map(|(name, _)| name), "the types held");
    let over: Vec<String> = sizes
        .iter()
        .zip(BUDGETS)
        .filter(|((_, size), (_, budget))| size > budget)
        .map(|((name, size), (_, budget))| format!("{name}: {size} bytes, budget {budget}"))
        .collect();
    assert!(over.is_empty(), "the app's state and its parts, in bytes: {over:#?}");
}

/// The budgets, in bytes: the sizes on 2026-11-13, before the moves, with Inner's 16 bytes more from main's merge that evening (`Panes`' plan keeping when
/// its zoom was asked, R3.6a), and Lifecycle's 48 more from the record's-lock retry: the launch held back from the updater (`LaunchFacts`' `held` and the
/// first frame's time beside the facts), the window `touched` since the launch, and the keeping state the life holds (`lifecycle::keeping`). One of each per
/// app, not per tab or page view.
const BUDGETS: [(&str, usize); 7] = [
    ("Inner", 4880),
    ("Lifecycle", 344),
    ("Serving", 1064),
    ("WindowParts", 464),
    ("Tools", 1440),
    ("Published", 808),
    ("AppState", 248),
];

/// Failure: a move widens what other code can reach of the app's state: a part's field made `pub`, a part given `Default` or `Clone` so it can be built
/// or replaced from outside, a method made `pub(crate)` that only its part should call; or takes away what a caller needs. The list is changed by hand, in
/// the commit that means it, so each widening and narrowing is in a diff.
#[test]
fn what_the_apps_state_lets_other_code_reach_is_listed() {
    let types = ["Inner", "Lifecycle", "Serving", "WindowParts", "Tools", "Published", "AppState"];
    let files = recto_testpdf::surface::rust_files(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/core"));
    let now = recto_testpdf::surface::listing(
        &files,
        recto_testpdf::surface::Scope {
            only: Some(&types),
            fields_of: &types,
        },
    );
    if let Err(changed) = recto_testpdf::surface::same_as(SURFACE, &now, "the app's state") {
        panic!("{changed}");
    }
}

/// What the app's state and its parts let other code reach, sorted. Each change is deliberate: see [`what_the_apps_state_lets_other_code_reach_is_listed`].
const SURFACE: &str = r"
AppState (pub struct, derives Clone, Debug)
AppState.pub app_hidden: bool
AppState.pub appearance: Appearance
AppState.pub contents_resume: Option<(TabId, ContentsScroll)>
AppState.pub contents_y: Option<f64>
AppState.pub focus: FocusRecord
AppState.pub from_state: Option<Box<AppState>>
AppState.pub hidden_decided: bool
AppState.pub minimised_decided: bool
AppState.pub selection_resume: Option<(TabId, record::Selection)>
AppState.pub selections_held: Vec<(usize, recto_index::select::Range)>
AppState.pub settings_window: Option<SettingsWindow>
AppState.pub sidebar_width: f64
AppState.pub window: Option<WindowRecord>
AppState.pub window_closed: bool
AppState.pub window_decided: bool
Inner (pub struct)
Inner.pub core: Arc<CoreQueue>
Inner.pub engine: Engine
Inner.pub host: Arc<AppHost>
Inner.pub life: Arc<Lifecycle>
Inner.pub panes: panes::Panes
Inner.pub published: Mutex<Published>
Inner.pub qa: Option<Arc<QaBridge>>
Inner.pub saver: Arc<Saver>
Inner.pub serving: Serving
Inner.pub tools: Tools
Inner.pub updates: Arc<recto_update::updater::Updater>
Inner.pub window: WindowParts
Inner.pub(crate) keys: Arc<recto_reader::engine::Keyring>
Inner.pub(crate) listener: Arc<dyn crate::Listener>
Inner.pub(crate) sleeper: sleeper::Sleeper
Inner::pub fn act(self: &Arc<Self>, action: UiAction) -> Acted
Inner::pub fn app_record(&self) -> AppRecord
Inner::pub fn bar_now(&self) -> Vec<crate::menus::MenuModel>
Inner::pub fn begin_restore(self: &Arc<Self>)
Inner::pub fn can_save(&self) -> bool
Inner::pub fn carry_out(&self, out: Option<crate::lifecycle::Out>)
Inner::pub fn check_woken_files(&self)
Inner::pub fn close_answered(self: &Arc<Self>, tab: u32, choice: CloseChoice) -> HostAction
Inner::pub fn close_prompt(&self) -> ClosePrompt
Inner::pub fn dismiss_selection_notice(&self)
Inner::pub fn dormancy_due(&self) -> Option<Vec<TabId>>
Inner::pub fn forget_a_quiet_copy(&self)
Inner::pub fn frontmost_changed(&self, app: bool, window_key: bool)
Inner::pub fn gather_record(&self) -> (SessionRecord, PlaceDelta)
Inner::pub fn give_sleep_view(&self, engine: &recto_reader::Engine, id: TabId)
Inner::pub fn go_dormant(&self, tabs: &[TabId], why: Why) -> usize
Inner::pub fn hand_off(&self, limit: Duration) -> Result<HandOffReport, HandOffError>
Inner::pub fn hand_off_cancelled(&self)
Inner::pub fn hand_off_check(&self) -> HandOffCheck
Inner::pub fn hand_off_quit_now(&self) -> bool
Inner::pub fn has_setting(&self, id: &str) -> bool
Inner::pub fn link_prompt_answered(&self, id: u64, open: bool) -> Option<String>
Inner::pub fn look_at_budget(&self)
Inner::pub fn memory_pressure(&self, critical: bool)
Inner::pub fn menu_item_enabled(&self, id: String) -> bool
Inner::pub fn note_input(&self)
Inner::pub fn note_nearby(&self)
Inner::pub fn note_update_facts(&self, version: String, dev_version: Option<String>, bundle_path: String, system_version: String)
Inner::pub fn notice_read(&self)
Inner::pub fn password_answered(&self, id: u64, password: Option<String>)
Inner::pub fn perform(self: &Arc<Self>, id: String) -> HostAction
Inner::pub fn qa_save_from(&self, written: &str) -> Result<(), AgentError>
Inner::pub fn qa_session_verb(&self, verb: &str, value: &str) -> Option<Result<(), AgentError>>
Inner::pub fn queue_those_leaving(&self)
Inner::pub fn quit_requested(self: &Arc<Self>)
Inner::pub fn quit_stayed(&self)
Inner::pub fn ready(&self, open: &[PathBuf])
Inner::pub fn ready_to_show(&self) -> bool
Inner::pub fn rearm_dormancy(&self)
Inner::pub fn recent_files(&self) -> Vec<RecentFile>
Inner::pub fn record_report(&self) -> Value
Inner::pub fn record_window_shown(&self, home: bool, tile_layers: u32, why: &str)
Inner::pub fn restore_patience_over(&self)
Inner::pub fn save_as(self: &Arc<Self>, tab: u32, path: String)
Inner::pub fn save_panel(&self) -> SavePanelModel
Inner::pub fn save_problem_answered(&self, save_as: bool) -> super::HostAction
Inner::pub fn set_frozen(&self, frozen: bool)
Inner::pub fn setting_chosen(self: &Arc<Self>, id: &str, on: bool)
Inner::pub fn settings_model(&self) -> SettingsModel
Inner::pub fn settings_window(&self) -> Option<SettingsWindow>
Inner::pub fn settings_window_changed(&self, place: Option<SettingsWindow>)
Inner::pub fn sheet_changed(&self, open: bool)
Inner::pub fn shown_with_state(&self)
Inner::pub fn shutdown(&self)
Inner::pub fn sidebar_tabs(&self) -> Vec<SidebarRow>
Inner::pub fn start(start: Start) -> Result<Arc<Self>, String>
Inner::pub fn start_with(start: Start, update_seams: updates::UpdateSeams) -> Result<Arc<Self>, String>
Inner::pub fn state_json(&self) -> Value
Inner::pub fn state_read(&self) -> bool
Inner::pub fn system_pressure(&self, critical: bool)
Inner::pub fn take_places(&self)
Inner::pub fn take_update_alert(&self) -> Option<UpdateAlert>
Inner::pub fn update_command(&self, command: &Command) -> Result<Value, AgentError>
Inner::pub fn update_report(&self) -> Value
Inner::pub fn update_test_clock(&self, ahead_ms: i64) -> bool
Inner::pub fn update_timer_fired(&self)
Inner::pub fn window_closed(&self)
Inner::pub fn window_interactive(&self) -> bool
Inner::pub fn window_opened(&self)
Inner::pub fn window_replaced(&self)
Inner::pub fn window_should_open(&self) -> bool
Inner::pub fn write_sleep_for(&self, id: TabId, by: By)
Inner::pub fn write_sleep_for_until(&self, id: TabId, until: Instant)
Inner::pub(crate) fn carry_out_quit(self: &Arc<Self>, out: Option<Out>)
Inner::pub(crate) fn carry_out_quit_answer(&self, out: Out)
Lifecycle (pub struct)
Lifecycle.pub first_shown: Mutex<Option<Value>>
Lifecycle.pub holding_tab: Mutex<Option<TabId>>
Lifecycle.pub sheets: AtomicU32
Lifecycle.pub shown_by_cap: AtomicBool
Lifecycle.pub started: Instant
Lifecycle.pub window_opened_at: Mutex<Option<Instant>>
Lifecycle.pub(crate) clock: Arc<dyn crate::clock::Clock>
Lifecycle.pub(crate) last_input_ms: AtomicU64
Lifecycle.pub(crate) others_looking: AtomicU32
Lifecycle.pub(crate) persons_running: AtomicU32
Lifecycle.pub(crate) qa_hold_reads: AtomicBool
Lifecycle.pub(crate) reads_running: AtomicU32
Lifecycle::pub fn may_remake_window(&self) -> bool
Lifecycle::pub fn new(clock: Arc<dyn crate::clock::Clock>) -> Self
Lifecycle::pub fn state_read(&self) -> bool
Lifecycle::pub fn step(&self, event: crate::lifecycle::Event) -> Option<crate::lifecycle::Out>
Lifecycle::pub(crate) fn due(&self) -> Option<Duration>
Lifecycle::pub(crate) fn now_ms(&self) -> u64
Lifecycle::pub(crate) fn remade(&self) -> bool
Lifecycle::pub(crate) fn shown_with_state(&self) -> bool
Lifecycle::pub(crate) fn takes_input(&self) -> bool
Lifecycle::pub(crate) fn told_to_quit(&self) -> bool
Published (pub struct)
Published.pub contents: Arc<Vec<ContentsEntry>>
Published.pub pane_tocs: Vec<Option<Arc<recto_core::Toc>>>
Published.pub rail_filled: u32
Published.pub reader: ReaderView
Published.pub recent: Arc<Vec<RecentFile>>
Published.pub recent_key: Option<(u64, u64)>
Published.pub sidebar: Arc<crate::SidebarView>
Published.pub tabs: Arc<Vec<crate::SidebarRow>>
Published::pub fn empty() -> Self
Serving (pub struct)
Serving.pub paths: Option<StatePaths>
Serving.pub(crate) announcer: Mutex<Option<Announcer>>
Serving.pub(crate) heard: Mutex<crate::testkit::heard::Overheard>
Serving.pub(crate) place_seams: crate::testkit::place_seams::PlaceSeams
Serving.pub(crate) socket: bool
Serving.pub(crate) stopper: Mutex<Option<Stopper>>
Serving.pub(crate) waiting_server: Mutex<Option<Server>>
Tools (pub struct)
Tools.pub chrome: crate::chrome::Chrome
Tools.pub folds: crate::sidebar_access::Folds
Tools.pub printing: printing::Printing
Tools.pub tool: snapshot_tool::Tool
Tools.pub(crate) find_field: Mutex<crate::find::Field>
Tools.pub(crate) palette: crate::palette::Palette
Tools.pub(crate) prompts: crate::prompts::Prompts
Tools.pub(crate) save_ui: saving::SaveUi
Tools::pub fn new(save_step: &Arc<AtomicU8>) -> Self
WindowParts (pub struct)
WindowParts.pub app: Mutex<AppState>
WindowParts.pub contents_follow: Mutex<ContentsFollow>
WindowParts.pub dark: AtomicBool
WindowParts.pub panel_open: AtomicBool
WindowParts.pub watchdog: Watchdog
WindowParts.pub(crate) action_notice: Mutex<Option<String>>
WindowParts.pub(crate) notice_tab: AtomicU32
WindowParts.pub(crate) open_failure: Mutex<Option<String>>
WindowParts::pub fn new(watchdog: Watchdog) -> Self
";
