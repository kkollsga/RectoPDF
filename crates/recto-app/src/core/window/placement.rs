//! **Where a new window goes,** on the screens there are now: Rust's rule, from what was saved, with what's decided for a launch kept (whether it comes back
//! in the Dock). Moved from the bridge's `window_placement` (R3.7, S2), its body unchanged but for the receiver and what it's passed.

use super::Window;
use crate::core::updates;
use crate::lock;
use crate::records::ScreenInfo;
use recto_session::record::{self, FocusView, Rect, ScreenNow, place_window};

/// Where a new window goes, as Rust's rule has it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowPlacement {
    /// There's a place to go: otherwise it's a new window, wherever the host starts one.
    pub saved: bool,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Go into full screen on showing. Only for an app that's come forward: it moves the user to another Space.
    pub full_screen: bool,
    /// Come forward. An app that was in the background when it quit reopens without taking the focus from anyone.
    pub activate: bool,
    pub focus_sidebar: bool,
    /// This is an update arriving with the user doing something else. Nothing goes over what they're looking at, and nothing's made key: the window goes behind everything, and takes it from there when they come to it.
    pub in_background: bool,
    /// And it was in full screen, which it goes back into when they come to it.
    pub full_screen_when_active: bool,
    /// It goes into the Dock, as it was there when the update came. Shown, it's where the rest of this says.
    pub minimised: bool,
}

impl Window {
    /// Where the new window goes, on the screens there are now, for a launch `reopened` by an update or not. In a `quiet` run it keeps its place off every
    /// screen, and only its size comes back.
    pub fn placement(&self, screens: Vec<ScreenInfo>, reopened: bool, quiet: bool) -> WindowPlacement {
        let app = {
            let mut app = lock(&self.app);
            // In the Dock across an update, as it was. Any other launch shows it, and the record's told so.
            if let Some(window) = app.window.as_mut() {
                window.minimised = updates::stays_out_of_sight(window.minimised, reopened);
            }
            app.minimised_decided = true;
            app.clone()
        };
        let minimised = app.window.as_ref().is_some_and(|window| window.minimised);
        let activate = app.focus.app_frontmost;
        let focus_sidebar = app.focus.view == FocusView::Sidebar;
        let in_background = updates::opens_in_background(reopened, activate);
        let Some(saved) = app.window else {
            return WindowPlacement {
                saved: false,
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
                full_screen: false,
                activate,
                focus_sidebar,
                in_background,
                full_screen_when_active: false,
                minimised: false,
            };
        };
        if quiet {
            return WindowPlacement {
                saved: true,
                x: -40_000.0,
                y: -40_000.0,
                width: saved.frame.width.max(record::WINDOW_MIN.0),
                height: saved.frame.height.max(record::WINDOW_MIN.1),
                full_screen: saved.full_screen,
                activate,
                focus_sidebar,
                in_background: false,
                full_screen_when_active: false,
                minimised,
            };
        }
        let now: Vec<ScreenNow> = screens
            .into_iter()
            .map(|screen| ScreenNow {
                id: screen.id,
                name: screen.name,
                frame: Rect {
                    x: screen.x,
                    y: screen.y,
                    width: screen.width,
                    height: screen.height,
                },
                visible: Rect {
                    x: screen.visible_x,
                    y: screen.visible_y,
                    width: screen.visible_width,
                    height: screen.visible_height,
                },
            })
            .collect();
        let place = place_window(&saved, &now);
        let (full_screen, full_screen_when_active) = updates::full_screen_on_showing(place.full_screen, in_background);
        WindowPlacement {
            saved: true,
            x: place.frame.x,
            y: place.frame.y,
            width: place.frame.width,
            height: place.frame.height,
            full_screen,
            activate,
            focus_sidebar,
            in_background,
            full_screen_when_active,
            minimised,
        }
    }
}
