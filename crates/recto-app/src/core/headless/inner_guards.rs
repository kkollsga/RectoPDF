//! **What `Inner`'s moves must not change, pinned before they start** (R3.7; AGENTS.md, "guards go in before a move"): the size of the app's state and of
//! each of its parts, and what each lets other code reach. What every control sends and the host is told is held by the coverage test's golden
//! (`coverage.rs`); what a person's action allocates on the window's thread, by `tests/inner_allocations.rs`, in a binary of its own for its allocator.

use crate::core::keeper::Keeper;
use crate::core::sleeper::{OnDisk, Sleeper};
use crate::core::window::Notices;
use crate::core::{AppState, Inner, Lifecycle, Published, Serving, Tools, Window};
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
        ("Window", size_of::<Window>()),
        ("Notices", size_of::<Notices>()),
        ("Tools", size_of::<Tools>()),
        ("Published", size_of::<Published>()),
        ("AppState", size_of::<AppState>()),
        ("Sleeper", size_of::<Sleeper>()),
        ("OnDisk", size_of::<OnDisk>()),
        ("Keeper", size_of::<Keeper>()),
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
/// first frame's time beside the facts), the window `touched` since the launch, and the keeping state the life holds (`lifecycle::keeping`); Inner's 184
/// more from the sleep branch's sleeper, held inline: the cool-down queue (`sleep_queue::Leaving`, where `main` had the queue under its lock alone) with its
/// condition variable, its wait, its writer's panic seam, a test's hold and the queue's own counts, the answer slots an agent's asked sleep waits on, the
/// ticket counter, and the hurry going to the background asks for; and Tools' and Inner's 8 more for Find's fingerprint of what the host was last told it
/// draws (`find::Field`, R3.7 S2's fix: Find announced with nothing changed). And the sleeper on its own, at its size on 2026-11-14 (608 bytes, inside
/// Inner's then), before R3.7 S5 moves it behind an `Arc` of its own, where Inner's budget no longer sees it. Then S5 itself: Inner 5,072 to 4,480 (the
/// sleeper's 608 out, `OnDisk`'s two `Arc`s, 16, in), and the record's keeper, 40 (five handles the app already has, held once more so the record's threads
/// hold it and not the app). And S2b's window's notices, 80 (its two locks, out of `Window`, whose size holds: the tab they're about stays in its padding). One
/// of each per app, not per tab or page view.
const BUDGETS: [(&str, usize); 11] = [
    ("Inner", 4480),
    ("Lifecycle", 344),
    ("Serving", 1064),
    ("Window", 464),
    ("Notices", 80),
    ("Tools", 1448),
    ("Published", 808),
    ("AppState", 248),
    ("Sleeper", 608),
    ("OnDisk", 16),
    ("Keeper", 40),
];

/// Failure: a move widens what other code can reach of the app's state: a part's field made `pub`, a part given `Default` or `Clone` so it can be built
/// or replaced from outside, a method made `pub(crate)` that only its part should call; or takes away what a caller needs. The list is changed by hand, in
/// the commit that means it, so each widening and narrowing is in a diff.
#[test]
fn what_the_apps_state_lets_other_code_reach_is_listed() {
    let types = [
        "Inner",
        "LaunchFacts",
        "Lifecycle",
        "Serving",
        "Window",
        "Notices",
        "Tools",
        "Printing",
        "Prompts",
        "Published",
        "AppState",
        "OnDisk",
        "Keeper",
    ];
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    // The core's files, and the prompts' own, whose type the tools hand to the bridge.
    let mut files = recto_testpdf::surface::rust_files(&src.join("core"));
    files.push(src.join("prompts.rs"));
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

/// What the app's state and its parts let other code reach, sorted, with printing's (R3.7, S2: its two `pub` fields the bridge reaches become methods) and the
/// prompts' (the bridge answers a sheet; the queue of passwords and the link's sheet are the app's). Each change is deliberate: see [`what_the_apps_state_lets_other_code_reach_is_listed`].
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
Inner.pub window: Window
Inner.pub(crate) keys: Arc<recto_reader::engine::Keyring>
Inner.pub(crate) listener: Arc<dyn crate::Listener>
Inner.pub(crate) on_disk: sleeper::OnDisk
Inner::pub fn act(self: &Arc<Self>, action: UiAction) -> Acted
Inner::pub fn bar_now(&self) -> Vec<crate::menus::MenuModel>
Inner::pub fn begin_restore(self: &Arc<Self>)
Inner::pub fn can_save(&self) -> bool
Inner::pub fn carry_out(&self, out: Option<crate::lifecycle::Out>)
Inner::pub fn close_answered(self: &Arc<Self>, tab: u32, choice: CloseChoice) -> HostAction
Inner::pub fn close_prompt(&self) -> ClosePrompt
Inner::pub fn dismiss_selection_notice(&self)
Inner::pub fn gather_record(&self) -> (SessionRecord, PlaceDelta)
Inner::pub fn hand_off(&self, limit: Duration) -> Result<HandOffReport, HandOffError>
Inner::pub fn hand_off_cancelled(&self)
Inner::pub fn hand_off_check(&self) -> HandOffCheck
Inner::pub fn hand_off_quit_now(&self) -> bool
Inner::pub fn menu_item_enabled(&self, id: String) -> bool
Inner::pub fn note_input(&self)
Inner::pub fn notice_read(&self)
Inner::pub fn perform(self: &Arc<Self>, id: String) -> HostAction
Inner::pub fn qa_save_from(&self, written: &str) -> Result<(), AgentError>
Inner::pub fn qa_session_verb(&self, verb: &str, value: &str) -> Option<Result<(), AgentError>>
Inner::pub fn quit_requested(self: &Arc<Self>)
Inner::pub fn quit_stayed(&self)
Inner::pub fn ready(&self, open: &[PathBuf])
Inner::pub fn recent_files(&self) -> Vec<RecentFile>
Inner::pub fn record_report(&self) -> Value
Inner::pub fn restore_patience_over(&self)
Inner::pub fn save_as(self: &Arc<Self>, tab: u32, path: String)
Inner::pub fn save_panel(&self) -> SavePanelModel
Inner::pub fn save_problem_answered(&self, save_as: bool) -> super::HostAction
Inner::pub fn set_frozen(&self, frozen: bool)
Inner::pub fn shown_with_state(&self)
Inner::pub fn shutdown(&self)
Inner::pub fn sidebar_tabs(&self) -> Vec<SidebarRow>
Inner::pub fn start(start: Start) -> Result<Arc<Self>, String>
Inner::pub fn start_with(start: Start, update_seams: updates::UpdateSeams) -> Result<Arc<Self>, String>
Inner::pub fn state_json(&self) -> Value
Inner::pub fn system_pressure(&self, critical: bool)
Inner::pub fn take_places(&self)
Inner::pub fn window_closed(&self)
Inner::pub fn window_opened(&self)
Inner::pub fn window_replaced(&self)
Inner::pub(crate) fn carry_out_quit(self: &Arc<Self>, out: Option<Out>)
Inner::pub(crate) fn carry_out_quit_answer(&self, out: Out)
Keeper (pub struct)
Keeper::pub(crate) fn asks(self: &Arc<Self>) -> KeepsRecord<'_>
Keeper::pub(crate) fn may_write(self: &Arc<Self>) -> bool
Keeper::pub(super) fn begin_restore(self: &Arc<Self>)
Keeper::pub(super) fn desk(self: &Arc<Self>) -> Desk<'_>
Keeper::pub(super) fn new(store: Arc<RecordStore>, life: Arc<Lifecycle>, host: Arc<AppHost>, core: Arc<CoreQueue>, updates: Arc<recto_update::updater::Updater>) -> Arc<Self>
Keeper::pub(super) fn refused(self: &Arc<Self>) -> Option<StoreError>
Keeper::pub(super) fn session(&self) -> &recto_control::host::SessionHost<recto_agent::DrawableLoader>
Keeper::pub(super) fn store(&self) -> &Arc<RecordStore>
Keeper::pub(super) fn try_the_lock(self: &Arc<Self>, verdict: std::sync::mpsc::Sender<Verdict>, heard: &std::sync::mpsc::Receiver<Verdict>)
LaunchFacts (pub struct)
LaunchFacts::pub fn first_frame_shown(&self, updates: &recto_update::updater::Updater)
LaunchFacts::pub fn note(&self, version: String, dev_version: Option<String>, bundle_path: String, system_version: String)
LaunchFacts::pub(super) fn held(&self) -> bool
LaunchFacts::pub(super) fn hold_or_tell(&self, keeps: bool, updates: &recto_update::updater::Updater) -> recto_update::updater::AtLaunch
LaunchFacts::pub(super) fn misreads(&self) -> bool
LaunchFacts::pub(super) fn new() -> Self
LaunchFacts::pub(super) fn tell(&self, updates: &recto_update::updater::Updater) -> recto_update::updater::AtLaunch
LaunchFacts::pub(super) fn untold(&self) -> bool
Lifecycle (pub struct)
Lifecycle.pub update_facts: updates::LaunchFacts
Lifecycle::pub fn may_remake_window(&self) -> bool
Lifecycle::pub fn new(clock: Arc<dyn crate::clock::Clock>) -> Self
Lifecycle::pub fn note_input(&self)
Lifecycle::pub fn note_nearby(&self)
Lifecycle::pub fn ready_to_show(&self, session: &SessionHost<DrawableLoader>, engine: &recto_reader::Engine) -> bool
Lifecycle::pub fn record_window_shown(&self, home: bool, tile_layers: u32, why: &str, session: &SessionHost<DrawableLoader>, engine: &recto_reader::Engine)
Lifecycle::pub fn seconds_since_start(&self) -> f64
Lifecycle::pub fn sheet_changed(&self, open: bool)
Lifecycle::pub fn state_read(&self) -> bool
Lifecycle::pub fn step(&self, event: crate::lifecycle::Event) -> Option<crate::lifecycle::Out>
Lifecycle::pub fn window_interactive(&self) -> bool
Lifecycle::pub(crate) fn due(&self) -> Option<Duration>
Lifecycle::pub(crate) fn first_shown(&self) -> Option<Value>
Lifecycle::pub(crate) fn hold_reads(&self, hold: bool)
Lifecycle::pub(crate) fn holding_tab(&self) -> Option<TabId>
Lifecycle::pub(crate) fn keeping_thread_out(&self) -> bool
Lifecycle::pub(crate) fn last_input_ms(&self) -> u64
Lifecycle::pub(crate) fn looking_at_others(&self)
Lifecycle::pub(crate) fn now_ms(&self) -> u64
Lifecycle::pub(crate) fn others_looked_at(&self)
Lifecycle::pub(crate) fn others_looking(&self) -> bool
Lifecycle::pub(crate) fn person_running(self: &Arc<Self>) -> PersonRunning
Lifecycle::pub(crate) fn persons_running(&self) -> u32
Lifecycle::pub(crate) fn read_finished(&self)
Lifecycle::pub(crate) fn read_started(&self)
Lifecycle::pub(crate) fn reads_held(&self) -> bool
Lifecycle::pub(crate) fn reads_running(&self) -> u32
Lifecycle::pub(crate) fn remade(&self) -> bool
Lifecycle::pub(crate) fn set_holding_tab(&self, tab: Option<TabId>)
Lifecycle::pub(crate) fn sheets_open(&self) -> u32
Lifecycle::pub(crate) fn shown_with_state(&self) -> bool
Lifecycle::pub(crate) fn take_holding_tab(&self) -> Option<TabId>
Lifecycle::pub(crate) fn told_to_quit(&self) -> bool
Lifecycle::pub(crate) fn window_opened(&self)
Lifecycle::pub(super) fn keeping(&self) -> crate::lifecycle::keeping::Keeping
Lifecycle::pub(super) fn keeping_due(&self) -> Option<Duration>
Lifecycle::pub(super) fn life(&self) -> crate::lifecycle::Life
Lifecycle::pub(super) fn step_keeping(&self, event: crate::lifecycle::keeping::Event, to: &mut super::keeping::Carrier<'_>)
Lifecycle::pub(super) fn tried(&self, got: bool, busy: impl FnOnce() -> bool, to: &mut super::keeping::Carrier<'_>)
Notices (pub struct)
Notices::pub(in crate::core) fn clear_open_failure(&self)
Notices::pub(in crate::core) fn fail_open(&self, words: String)
Notices::pub(in crate::core) fn notice(&self) -> Option<String>
Notices::pub(in crate::core) fn open_failure(&self) -> Option<String>
Notices::pub(in crate::core) fn say(&self, words: String)
Notices::pub(in crate::core) fn take_notice(&self) -> Option<String>
Notices::pub(super) fn new() -> Self
OnDisk (pub struct)
OnDisk::pub(crate) fn downgrade(&self) -> LetGo
OnDisk::pub(crate) fn keeper(&self) -> &Arc<Keeper>
OnDisk::pub(crate) fn sleeper(&self) -> &Arc<Sleeper>
OnDisk::pub(super) fn new(keeper: Arc<Keeper>, sleeper: Arc<Sleeper>) -> Self
OnDisk::pub(super) fn starts_writer(&self) -> StartsWriter<'_>
Printing (pub struct, derives Default)
Printing::pub fn busy(&self) -> bool
Printing::pub fn cancel(&self)
Printing::pub fn dismiss_notice(&self)
Printing::pub fn finished(&self, path: &str)
Printing::pub fn has_ready(&self) -> bool
Printing::pub fn notice(&self) -> Option<String>
Printing::pub fn preparing(&self) -> bool
Printing::pub fn qa_target_for(&self, out: PathBuf) -> Option<PathBuf>
Printing::pub fn quitting(&self)
Printing::pub fn selection_notice(&self) -> Option<SelectionNotice>
Printing::pub fn take_ready(&self) -> Option<PathBuf>
Prompts (pub struct, derives Debug, Default)
Prompts.pub(crate) link: Mutex<Option<LinkPrompt>>
Prompts.pub(crate) passwords: Mutex<Asks>
Prompts::pub fn link_answered(&self, id: u64, open: bool, core: &CoreQueue) -> Option<String>
Prompts::pub fn password_answered(&self, id: u64, password: Option<String>, core: &Arc<CoreQueue>, window: &Window)
Prompts::pub(crate) fn ask_link(&self, address: &str)
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
Tools::pub fn chrome(&self) -> &crate::chrome::Chrome
Tools::pub fn folds(&self) -> &crate::sidebar_access::Folds
Tools::pub fn new(save_step: &Arc<AtomicU8>) -> Self
Tools::pub fn printing(&self) -> &printing::Printing
Tools::pub fn prompts(&self) -> &crate::prompts::Prompts
Tools::pub fn tool(&self) -> &snapshot_tool::Tool
Tools::pub(crate) fn find_field(&self) -> &Mutex<crate::find::Field>
Tools::pub(crate) fn palette(&self) -> &crate::palette::Palette
Tools::pub(crate) fn save_ui(&self) -> &saving::SaveUi
Window (pub struct)
Window.pub(super) app: Mutex<AppState>
Window.pub(super) contents_follow: Mutex<ContentsFollow>
Window.pub(super) dark: AtomicBool
Window.pub(super) panel_open: AtomicBool
Window::pub fn app_record(&self, notice: impl FnOnce() -> Option<String>) -> AppRecord
Window::pub fn appearance_choice(&self) -> &'static str
Window::pub fn changed(&self, frame: Option<Vec<f64>>, screen: Option<ScreenInfo>, full_screen: bool, saver: &Saver)
Window::pub fn comes_back_hidden(&self, reopened: bool) -> bool
Window::pub fn contents_following(&self) -> bool
Window::pub fn contents_moving(&self) -> bool
Window::pub fn contents_pointer(&self, inside: bool)
Window::pub fn contents_position(&self, y: f64, saver: &Saver)
Window::pub fn contents_reached(&self, row: Option<u32>, view: ContentsView, now: f64, reduce_motion: bool) -> FollowMove
Window::pub fn contents_row_change(&self, change: ContentsRowChange, scroll_y: f64) -> Option<f64>
Window::pub fn contents_shifted(&self, by: f64)
Window::pub fn contents_step(&self, position: f64, dt: f64) -> Option<f64>
Window::pub fn contents_user_scrolled(&self, now: f64)
Window::pub fn contents_view_changed(&self)
Window::pub fn focus_changed(&self, view: &str, saver: &Saver)
Window::pub fn focus_view(&self) -> &'static str
Window::pub fn frontmost_changed(&self, app: bool, window_key: bool, saver: &Saver, core: &CoreQueue)
Window::pub fn hidden_changed(&self, hidden: bool, saver: &Saver)
Window::pub fn is_closed(&self) -> bool
Window::pub fn main_busy(&self, block: bool) -> bool
Window::pub fn main_idle(&self)
Window::pub fn minimised_changed(&self, minimised: bool, saver: &Saver)
Window::pub fn new(watchdog: Watchdog) -> Self
Window::pub fn opens_in_background(&self, reopened: bool) -> bool
Window::pub fn panel_open(&self) -> bool
Window::pub fn placement(&self, screens: Vec<ScreenInfo>, reopened: bool, quiet: bool) -> WindowPlacement
Window::pub fn set_appearance_choice(&self, choice: &str, saver: &Saver)
Window::pub fn set_dark(&self, dark: bool, core: &CoreQueue)
Window::pub fn set_sidebar_width(&self, width: f64, core: &CoreQueue)
Window::pub fn settings_window(&self) -> Option<SettingsWindow>
Window::pub fn settings_window_changed(&self, place: Option<SettingsWindow>, saver: &Saver)
Window::pub fn should_open(&self, reopened: bool) -> bool
Window::pub fn showing(&self) -> bool
Window::pub(super) fn closed(&self)
Window::pub(super) fn notices(&self) -> &Notices
Window::pub(super) fn opened(&self)
Window::pub(super) fn replaced(&self, held: Vec<(usize, recto_index::select::Range)>)
Window::pub(super) fn shows_tab(&self, tab: u32) -> bool
Window::pub(super) fn take_selections_held(&self) -> Vec<(usize, recto_index::select::Range)>
";
