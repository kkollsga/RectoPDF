//! **The window as Rust keeps it:** what the host tells of it for the record (its frame, the Dock, hidden, the sidebar's width, the contents' scroll, focus,
//! appearance, the Settings window), the contents' follow, the UI thread's hang watchdog, and the notices that go by themselves. Its opening, replacing and
//! closing are `Inner`'s, as they reach the page views and the saver, and they call what's here.

use super::saver::Saver;
use crate::lock;
use crate::watchdog::Watchdog;
use recto_session::record::{self, AppRecord, Appearance, ContentsScroll, FocusRecord, SettingsWindow, WindowRecord};
use recto_session::{ContentsFollow, TabId};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// **What the window is and says:** what the host tells of it for the record, its appearance and panel, the contents' follow, the UI thread's hang watchdog, and the
/// notices that go by themselves.
pub struct Window {
    pub contents_follow: Mutex<ContentsFollow>,
    pub app: Mutex<AppState>,
    pub watchdog: Watchdog,
    pub dark: AtomicBool,
    pub panel_open: AtomicBool,
    pub(crate) open_failure: Mutex<Option<String>>,
    /// Why something the reader chose couldn't be done, or what a save has to say, as a notice that goes by itself.
    pub(crate) action_notice: Mutex<Option<String>>,
    /// The tab showing when the window was last brought up to date: a notice is about what was on screen, so it goes with a change of tab.
    pub(crate) notice_tab: AtomicU32,
}

impl Window {
    /// At launch: no window yet, the system's appearance, nothing said.
    pub fn new(watchdog: Watchdog) -> Self {
        Self {
            app: Mutex::new(AppState {
                window: None,
                appearance: Appearance::System,
                sidebar_width: record::SIDEBAR_WIDTH,
                contents_y: None,
                contents_resume: None,
                selection_resume: None,
                focus: FocusRecord::default(),
                settings_window: None,
                window_closed: false,
                app_hidden: false,
                hidden_decided: false,
                minimised_decided: false,
                window_decided: false,
                from_state: None,
                selections_held: Vec::new(),
            }),
            dark: AtomicBool::new(false),
            panel_open: AtomicBool::new(false),
            contents_follow: Mutex::new(ContentsFollow::default()),
            watchdog,
            open_failure: Mutex::new(None),
            action_notice: Mutex::new(None),
            notice_tab: AtomicU32::new(0),
        }
    }

    /// The host's own part of the record, with the notice the session as a whole has now.
    pub fn app_record(&self, notice: Option<String>) -> AppRecord {
        let app = lock(&self.app).clone();
        let following = lock(&self.contents_follow).is_following();
        AppRecord {
            window: app.window,
            appearance: app.appearance,
            sidebar_width: Some(app.sidebar_width),
            panel_open: self.panel_open.load(Ordering::SeqCst),
            contents_scroll: Some(ContentsScroll {
                y: app.contents_y.unwrap_or(0.0),
                following,
            }),
            focus: app.focus,
            notice,
            settings_window: app.settings_window,
            window_closed: app.window_closed,
            app_hidden: app.app_hidden,
        }
    }

    /// The Settings window opened, moved or closed: it's part of what comes back, so the record's told.
    pub fn settings_window_changed(&self, place: Option<SettingsWindow>, saver: &Saver) {
        lock(&self.app).settings_window = place;
        saver.view_changed();
    }

    /// Whether this launch opens a window, `reopened` by an update or not. Any launch does but an update's arrival, which leaves things as they were: a window
    /// closed stays closed, rather than appearing behind whatever the user's doing.
    pub fn should_open(&self, reopened: bool) -> bool {
        let mut app = lock(&self.app);
        app.window_decided = true;
        !(app.window_closed && reopened)
    }

    /// Where the Settings window was, if it was open.
    pub fn settings_window(&self) -> Option<SettingsWindow> {
        lock(&self.app).settings_window
    }
}

/// What the host tells Rust about the window, kept for the record. A short lock: the UI thread sets these and a writer copies them.
#[derive(Clone, Debug)]
pub struct AppState {
    pub window: Option<WindowRecord>,
    pub appearance: Appearance,
    pub sidebar_width: f64,
    /// The active tab's contents. A copy, as they're the host's to scroll.
    pub contents_y: Option<f64>,
    /// Where they go on the tab's first showing, and whose they are then.
    pub contents_resume: Option<(TabId, ContentsScroll)>,
    /// What was selected, to put back on the tab's first showing.
    pub selection_resume: Option<(TabId, record::Selection)>,
    pub focus: FocusRecord,
    pub settings_window: Option<SettingsWindow>,
    pub window_closed: bool,
    /// Hidden with ⌘H, as the record has it.
    pub app_hidden: bool,
    /// The host has asked whether the app comes back hidden, and done it: a restore after that keeps what it did.
    pub hidden_decided: bool,
    /// The same for the window in the Dock.
    pub minimised_decided: bool,
    /// And for whether there's a window at all.
    pub window_decided: bool,
    /// What the state said, as it was taken in after a window went up without it. That window can report its own size, focus and the rest before it's made again, which a full check on a slow machine saw, so the one made again goes by this: see [`AppCore::window_replaced`].
    pub from_state: Option<Box<AppState>>,
    /// What each pane had selected as its window went for another made from the state, to be selected again on the new one.
    pub selections_held: Vec<(usize, recto_index::select::Range)>,
}
