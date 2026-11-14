//! **The contents' follow, as the window keeps it:** what the host tells of the person scrolling and pointing at the contents, where a move goes next, and
//! a change of the tab being read. Moved from `window.rs` (R3.7, S2b) so the window's own block stays under the methods-per-`impl` budget, its bodies
//! unchanged.

use super::Window;
use crate::lock;
use recto_session::{ContentsRowChange, ContentsView, FollowMove, TabId};

impl Window {
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

    /// **Another tab is being read** (`active`): the contents are another tab's, and go back to where they were if that's the tab they were saved on, once
    /// (a restore's resume goes either way).
    pub(in crate::core) fn follow_tab(&self, active: Option<TabId>) {
        let mut follow = lock(&self.contents_follow);
        follow.new_contents();
        let mut app = lock(&self.app);
        match app.contents_resume.take() {
            Some((tab, scroll)) if Some(tab) == active => follow.resume_at(scroll.y, scroll.following),
            _ => {}
        }
    }
}
