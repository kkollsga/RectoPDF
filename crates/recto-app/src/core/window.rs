//! **The window as Rust keeps it:** what the host tells of it for the record (its frame, the Dock, hidden, the sidebar's width, the contents' scroll, focus,
//! appearance, the Settings window), the contents' follow, the UI thread's hang watchdog, and the notices that go by themselves. Its opening, replacing and
//! closing are `Inner`'s, as they reach the page views and the saver, and they call what's here.

mod placement;
pub use placement::WindowPlacement;

use super::saver::Saver;
use super::updates;
use crate::lock;
use crate::queue::{CoreCall, CoreQueue};
use crate::records::ScreenInfo;
use crate::watchdog::Watchdog;
use recto_session::record::{self, AppRecord, Appearance, ContentsScroll, FocusRecord, FocusView, Rect, ScreenRecord, SettingsWindow, WindowRecord};
use recto_session::{ContentsFollow, ContentsRowChange, ContentsView, FollowMove, TabId};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// **What the window is and says:** what the host tells of it for the record, its appearance and panel, the contents' follow, the UI thread's hang watchdog, and the
/// notices that go by themselves.
pub struct Window {
    /// The contents' follow, which the window's opening and the session thread's look at the contents set as well.
    pub(super) contents_follow: Mutex<ContentsFollow>,
    /// What the host tells of the window, for the record. Reached from outside the core only by the bridge's `window_placement`, which moves here next
    /// (R3.7 S2, once `build/record-lock-retry` is in), and by the headless kit's launch.
    pub app: Mutex<AppState>,
    watchdog: Watchdog,
    /// The system's appearance is dark, for the paper's look.
    pub(super) dark: AtomicBool,
    /// The sidebar's panel is open: its row toggles it and a restore sets it.
    pub(super) panel_open: AtomicBool,
    pub(super) open_failure: Mutex<Option<String>>,
    /// Why something the reader chose couldn't be done, or what a save has to say, as a notice that goes by itself.
    pub(super) action_notice: Mutex<Option<String>>,
    /// The tab showing when the window was last brought up to date: a notice is about what was on screen, so it goes with a change of tab.
    pub(super) notice_tab: AtomicU32,
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

    /// The app became the active one or stopped, and the window key or not. Going to the background writes what's changed: from there, nobody's watching.
    pub fn frontmost_changed(&self, app: bool, window_key: bool, saver: &Saver, core: &CoreQueue) {
        let mut state = lock(&self.app);
        let went_back = state.focus.app_frontmost && !app;
        let came_back = !state.focus.app_frontmost && app;
        state.focus.app_frontmost = app;
        state.focus.window_key = window_key;
        drop(state);
        if went_back {
            saver.write_soon()
        } else {
            saver.view_changed()
        }
        // Back from another app, which may have moved, replaced or deleted a file that's showing: each is looked at, a stat on a thread of its own, and said if it's something else now.
        if came_back {
            core.push(CoreCall::LookAtShown);
        }
    }

    /// The window went into the Dock or came out of it. Off the window's delegate, or QA.
    pub fn minimised_changed(&self, minimised: bool, saver: &Saver) {
        if let Some(window) = lock(&self.app).window.as_mut() {
            window.minimised = minimised;
        }
        saver.view_changed();
    }

    /// The app was hidden or shown again, by the system's notices, or QA. Going out of sight writes what's changed, as going to the background does.
    pub fn hidden_changed(&self, hidden: bool, saver: &Saver) {
        lock(&self.app).app_hidden = hidden;
        if hidden {
            saver.write_soon();
        } else {
            saver.view_changed();
        }
    }

    /// Whether this launch, `reopened` by an update or not, hides the app as soon as it's up: an update arriving to an app that was hidden. Any other launch
    /// shows it, and the record's told so.
    pub fn comes_back_hidden(&self, reopened: bool) -> bool {
        let mut app = lock(&self.app);
        let hidden = updates::stays_out_of_sight(app.app_hidden, reopened);
        app.app_hidden = hidden;
        app.hidden_decided = true;
        hidden
    }

    /// Whether this launch, `reopened` by an update or not, comes up behind what the user's doing: an update arriving to an app that wasn't in front.
    pub fn opens_in_background(&self, reopened: bool) -> bool {
        updates::opens_in_background(reopened, lock(&self.app).focus.app_frontmost)
    }

    /// The window moved, resized or went full screen, on this `screen`. Off a window's delegate, so at most as often as those. A window not told of yet is made
    /// at the default frame; a `frame` that isn't four numbers leaves the frame and screen as they were.
    pub fn changed(&self, frame: Option<Vec<f64>>, screen: Option<ScreenInfo>, full_screen: bool, saver: &Saver) {
        let mut app = lock(&self.app);
        let saved = app.window.get_or_insert(WindowRecord {
            frame: Rect {
                x: 0.0,
                y: 0.0,
                width: 1440.0,
                height: 900.0,
            },
            screen: None,
            full_screen: false,
            minimised: false,
        });
        // Its frame in full screen isn't where leaving full screen takes it.
        if let Some([x, y, width, height]) = frame.as_deref() {
            saved.frame = Rect {
                x: *x,
                y: *y,
                width: *width,
                height: *height,
            };
            saved.screen = screen.map(|screen| ScreenRecord {
                id: screen.id,
                name: recto_core::clean_line(&screen.name),
                frame: Rect {
                    x: screen.x,
                    y: screen.y,
                    width: screen.width,
                    height: screen.height,
                },
            });
        }
        saved.full_screen = full_screen;
        drop(app);
        saver.view_changed();
    }

    /// The sidebar's edge was dragged, or QA said so. Rust holds it to its limits, and the window's drawn again at it.
    pub fn set_sidebar_width(&self, width: f64, core: &CoreQueue) {
        lock(&self.app).sidebar_width = record::sidebar_width(width);
        core.push(CoreCall::Reconcile);
    }

    /// Where the contents are scrolled to now: nowhere, for what isn't a number.
    pub fn contents_position(&self, y: f64, saver: &Saver) {
        lock(&self.app).contents_y = y.is_finite().then_some(y);
        saver.view_changed();
    }

    /// What has the keyboard, by its name. A name that's none of them changes nothing.
    pub fn focus_changed(&self, view: &str, saver: &Saver) {
        if let Some(view) = FocusView::from_name(view) {
            lock(&self.app).focus.view = view;
            saver.view_changed();
        }
    }

    /// The name of what has the keyboard.
    pub fn focus_view(&self) -> &'static str {
        lock(&self.app).focus.view.name()
    }

    /// Light or dark as chosen, or `system`.
    pub fn appearance_choice(&self) -> &'static str {
        match lock(&self.app).appearance {
            Appearance::System => "system",
            Appearance::Light => "light",
            Appearance::Dark => "dark",
        }
    }

    /// Light or dark chosen, by its name; anything else is the system's.
    pub fn set_appearance_choice(&self, choice: &str, saver: &Saver) {
        lock(&self.app).appearance = match choice {
            "light" => Appearance::Light,
            "dark" => Appearance::Dark,
            _ => Appearance::System,
        };
        saver.view_changed();
    }

    /// The system's appearance went dark or light, and the window's drawn again in it.
    pub fn set_dark(&self, dark: bool, core: &CoreQueue) {
        self.dark.store(dark, Ordering::SeqCst);
        core.push(CoreCall::Reconcile);
    }

    /// The reader scrolled the contents themselves, `now` seconds into the launch.
    pub fn contents_user_scrolled(&self, now: f64) {
        lock(&self.contents_follow).user_scrolled(now);
    }

    /// The pointer came into the contents or left them.
    pub fn contents_pointer(&self, inside: bool) {
        lock(&self.contents_follow).pointer(inside);
    }

    /// The place being read is now on this row of the contents, as they are in `view`: what to do about it.
    pub fn contents_reached(&self, row: Option<u32>, view: ContentsView, now: f64, reduce_motion: bool) -> FollowMove {
        lock(&self.contents_follow).reached(row, view, now, reduce_motion)
    }

    /// One frame of a move: where the scroll goes next, or nothing when it's over and the frames should stop. An atomic-sized lock and some arithmetic.
    pub fn contents_step(&self, position: f64, dt: f64) -> Option<f64> {
        lock(&self.contents_follow).step(position, dt)
    }

    /// The contents' area changed height.
    pub fn contents_view_changed(&self) {
        lock(&self.contents_follow).view_changed();
    }

    /// Rows closed or opened: how many points to move the scroll by with no animation, or nothing if the rows are to change in view.
    pub fn contents_row_change(&self, change: ContentsRowChange, scroll_y: f64) -> Option<f64> {
        lock(&self.contents_follow).row_change(change, scroll_y)
    }

    /// Rows above changed, and the host moved its scroll by this many points to match.
    pub fn contents_shifted(&self, by: f64) {
        lock(&self.contents_follow).shifted(by);
    }

    /// Whether the contents follow the place being read.
    pub fn contents_following(&self) -> bool {
        lock(&self.contents_follow).is_following()
    }

    /// Whether the contents are moving to it now.
    pub fn contents_moving(&self) -> bool {
        lock(&self.contents_follow).is_moving()
    }

    /// The UI thread has woken, or (`block`) a block of the host's begins on it: true if this began a stretch the hang watchdog times.
    pub fn main_busy(&self, block: bool) -> bool {
        self.watchdog.enter(block)
    }

    /// The UI thread is about to sleep.
    pub fn main_idle(&self) {
        self.watchdog.idle();
    }

    /// Whether the sidebar's panel is open.
    pub fn panel_open(&self) -> bool {
        self.panel_open.load(Ordering::SeqCst)
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

#[cfg(test)]
mod tests;
