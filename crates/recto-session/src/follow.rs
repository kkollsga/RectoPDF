//! How the contents follow the place being read: a state machine with its times passed in, so the host only runs frames.

use crate::CONTENTS_ROW_HEIGHT;

/// How long after the reader stops using the contents, with the pointer out of them, before they may follow again.
pub const FOLLOW_RESUME_AFTER: f64 = 3.0;
/// Where the place being read comes to rest, as a part of the contents' height from the top. Above the middle, as reading goes downwards: there's more ahead to see than behind, and a longer rest before it needs to move.
pub(crate) const FOLLOW_ANCHOR: f64 = 0.3;
/// The dead zone, as parts of the contents' height from the top. Inside it the list doesn't move by one pixel, so reading slowly gets long rests. Its lower edge is what sets how long and how far: from the anchor to here is 45% of the height, about 12 rows in a 776 point list, and each move covers that much rather than nudging at every section. Its upper edge lets the reader step back a few rows with nothing moving.
pub(crate) const FOLLOW_DEAD_ZONE: (f64, f64) = (0.12, 0.75);
/// How long a move takes to settle, in seconds, shortest and longest: it grows with the distance and stays bounded, so a far jump is a fast whoosh that still starts and ends softly. Owner, 2026-11-07: "It can whoosh with smooth acceleration at both ends."
pub(crate) const FOLLOW_SECONDS: (f64, f64) = (0.6, 1.1);
/// The integration step, so a move's shape doesn't depend on the frame rate. At most a few of these a frame.
const FOLLOW_STEP: f64 = 1.0 / 240.0;
/// Close enough to be there, in points, and slow enough to stop, in points a second.
const FOLLOW_ARRIVED: (f64, f64) = (0.25, 3.0);

/// The contents' scrolling area, as it is when it's asked about.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContentsView {
    pub scroll_y: f64,
    pub height: f64,
    pub rows: u32,
}

/// What the host should do with the contents' scroll.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FollowMove {
    Stay,
    /// Go there at once: a tab's first showing, or Reduce Motion.
    SetTo(f64),
    /// Run frames and call [`ContentsFollow::step`].
    Move,
}

/// **How the contents follow the place being read.** Owner, 2026-11-07: "It can whoosh with smooth acceleration at both ends. It can have a "dead-zone" so it doesn't react to small scrolls within the window, so basically it repositions, stays still (while user is reading slowly scrolling) then repositions after the user is scrolling a bit." A plain state machine with its times passed in, so the host only runs frames.
///
/// - **Reposition, then rest.** The place sits at `FOLLOW_ANCHOR`. While it stays in `FOLLOW_DEAD_ZONE`, nothing moves. Leaving it starts one move, back to the anchor.
/// - **Every move starts and ends softly,** with its length bounded by `FOLLOW_SECONDS`. It's a chain: a lag on the target, followed by a critically damped spring. That's what makes acceleration build from nothing at the start, and fade at the end.
/// - **A new target during a move only changes where the chain is heading,** so nothing steps, in position, speed or acceleration. Fast reading through many sections carries on as one long move until it settles. It only changes once the place has left the dead zone around where the move is going.
/// - **A reader who scrolls the contents is left alone,** and so is a pointer resting in them. Following resumes on the next change of section once the pointer's out and [`FOLLOW_RESUME_AFTER`] has passed.
/// - **With Reduce Motion nothing animates:** it goes straight there, and only when the place has left the view.
///
/// Costs a few comparisons a section, and nothing per row. The host's frames run only during a move.
#[derive(Clone, Copy, Debug)]
pub struct ContentsFollow {
    paused: bool,
    pointer_inside: bool,
    last_user_scroll: f64,
    current: Option<u32>,
    placed: bool,
    moving: Option<Motion>,
    /// Where to go on the first showing, as a restore found it, and whether it was following.
    resume: Option<(f64, bool)>,
}

/// Which rows of the contents closed and opened, and how far that carried the place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentsRowChange {
    pub from: u32,
    pub removed: u32,
    pub inserted: u32,
    pub place_shift: i32,
}

/// How much faster a move gets each time a section goes by with the place out of the view.
const FOLLOW_CATCH_UP: f64 = 1.15;
/// The chain's fastest rate, per second: its lag behind what it's chasing is about 3 divided by this.
const FOLLOW_FASTEST_RATE: f64 = 120.0;

/// A move under way.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Motion {
    target: f64,
    /// The lagged target the spring chases, which is what makes it start softly.
    aim: f64,
    /// Where it really is, in the chain. A scroll view rounds to whole device pixels, and a slow end reading that back would never arrive. Taken from the host on the first frame.
    at: Option<f64>,
    velocity: f64,
    /// Rate of the chain, per second, from the distance.
    rate: f64,
}

impl Default for ContentsFollow {
    fn default() -> Self {
        Self {
            paused: false,
            pointer_inside: false,
            last_user_scroll: f64::NEG_INFINITY,
            current: None,
            placed: false,
            moving: None,
            resume: None,
        }
    }
}

/// The chain's rate for a distance: the further, the longer it takes, up to the most, so far jumps are quick and short ones aren't abrupt.
fn follow_rate(distance: f64, height: f64) -> f64 {
    let heights = distance.abs() / height.max(1.0);
    let share = (heights / 4.0).clamp(0.0, 1.0).sqrt();
    let seconds = FOLLOW_SECONDS.0 + (FOLLOW_SECONDS.1 - FOLLOW_SECONDS.0) * share;
    // A chain like this takes this many of its natural periods to close from this far to the last quarter of a point, so a long way is no longer than the time above.
    let periods = 6.0 + (distance.abs().max(1.0) / FOLLOW_ARRIVED.0).ln();
    periods / seconds
}

impl ContentsFollow {
    /// The reader scrolled the contents. Whatever was moving stops, right where it is.
    pub fn user_scrolled(&mut self, now: f64) {
        self.paused = true;
        self.last_user_scroll = now;
        self.moving = None;
    }

    /// The pointer came into the contents or left. Nothing's pulled from under it: a move under way stops.
    pub fn pointer(&mut self, inside: bool) {
        self.pointer_inside = inside;
        if inside {
            self.moving = None;
        }
    }

    /// Another tab: its contents start fresh, in the right place.
    pub fn new_contents(&mut self) {
        let pointer_inside = self.pointer_inside;
        *self = Self {
            pointer_inside,
            ..Self::default()
        };
    }

    /// The first showing goes here, as it was left, and not to where the place would put it. Whether they were following or left by the reader comes back too.
    pub fn resume_at(&mut self, y: f64, following: bool) {
        if y.is_finite() {
            self.resume = Some((y.max(0.0), following));
            self.paused = !following;
        }
    }

    /// The contents' area changed height, as the window did. The place is looked at again next, and goes straight to where it should be, since nothing about a resize wants moving.
    pub fn view_changed(&mut self) {
        self.moving = None;
        // A list the reader has scrolled themselves stays where they left it, resized or not. Clearing this would count as a change of section, and snap it back once they'd stopped.
        if self.paused {
            return;
        }
        self.current = None;
        self.placed = false;
    }

    pub fn is_following(self) -> bool {
        !self.paused
    }

    pub fn is_moving(self) -> bool {
        self.moving.is_some()
    }

    /// Rows closed or opened. Following, the place stays where it is on screen, so that a chapter closing above it isn't a second move straight after the first: what's above it changes, and the scroll takes it up. Otherwise it's what's at the top that stays, where the reader's looking. Says how many points to move the scroll by, with no animation, or `None` if the rows are to slide, since they're in the view and there's nothing to take up.
    pub fn row_change(&self, change: ContentsRowChange, scroll_y: f64) -> Option<f64> {
        let following = self.placed && !self.paused && !self.pointer_inside;
        if following && change.place_shift != 0 {
            return Some(f64::from(change.place_shift) * CONTENTS_ROW_HEIGHT);
        }
        let above = f64::from(change.from) + f64::from(change.removed);
        (scroll_y / CONTENTS_ROW_HEIGHT >= above).then(|| (f64::from(change.inserted) - f64::from(change.removed)) * CONTENTS_ROW_HEIGHT)
    }

    /// Rows above something were added or removed, by this many points, and the host moved its scroll to match so nothing appears to jump. The whole of a move goes with them.
    pub fn shifted(&mut self, by: f64) {
        if let Some(motion) = &mut self.moving {
            motion.target += by;
            motion.aim += by;
            motion.at = motion.at.map(|at| at + by);
        }
    }

    /// The place being read is now on this row. `now` is in seconds, from any fixed moment.
    pub fn reached(&mut self, row: Option<u32>, view: ContentsView, now: f64, reduce_motion: bool) -> FollowMove {
        if self.current == row {
            return FollowMove::Stay;
        }
        self.current = row;
        // Folded: there's nothing to follow, and what's under way stops. Opened again, the place is straight where it should be, and nothing whooshes from the top.
        let Some(row) = row else {
            self.moving = None;
            self.placed = false;
            self.paused = false;
            return FollowMove::Stay;
        };
        if self.paused {
            if self.pointer_inside || now - self.last_user_scroll < FOLLOW_RESUME_AFTER {
                return FollowMove::Stay;
            }
            self.paused = false;
        }
        let place = (f64::from(row) + 0.5) * CONTENTS_ROW_HEIGHT;
        let furthest = (f64::from(view.rows) * CONTENTS_ROW_HEIGHT - view.height).max(0.0);
        let target = (place - view.height * FOLLOW_ANCHOR).clamp(0.0, furthest);
        if !self.placed {
            self.placed = true;
            if let Some((y, following)) = self.resume.take() {
                self.paused = !following;
                return FollowMove::SetTo(y.min(furthest));
            }
            return FollowMove::SetTo(target);
        }
        if self.pointer_inside {
            return FollowMove::Stay;
        }
        if reduce_motion {
            self.moving = None;
            let in_view = place >= view.scroll_y && place <= view.scroll_y + view.height;
            return if in_view { FollowMove::Stay } else { FollowMove::SetTo(target) };
        }
        // A move under way counts from where it's going, so sections going by fast are one long move rather than a row of them, and sections going by slowly are a move and then a rest. If the place has got out of the view itself, the view is trailing: it goes for where the place is now, and faster each time, or scrolling past the speed of the move would lose the place for good.
        if let Some(motion) = &mut self.moving {
            let zone = (motion.target + view.height * FOLLOW_DEAD_ZONE.0)..=(motion.target + view.height * FOLLOW_DEAD_ZONE.1);
            let at = motion.at.unwrap_or(view.scroll_y);
            let in_view = (at..=at + view.height).contains(&place);
            if zone.contains(&place) && in_view {
                return FollowMove::Move;
            }
            motion.target = target;
            if !in_view {
                motion.rate = (motion.rate * FOLLOW_CATCH_UP).min(FOLLOW_FASTEST_RATE);
            }
            return FollowMove::Move;
        }
        let zone = (view.scroll_y + view.height * FOLLOW_DEAD_ZONE.0)..=(view.scroll_y + view.height * FOLLOW_DEAD_ZONE.1);
        if zone.contains(&place) || (target - view.scroll_y).abs() < 1.0 {
            return FollowMove::Stay;
        }
        self.moving = Some(Motion {
            target,
            aim: view.scroll_y,
            at: Some(view.scroll_y),
            velocity: 0.0,
            rate: follow_rate(target - view.scroll_y, view.height),
        });
        FollowMove::Move
    }

    /// One frame of a move. Returns where the scroll should be, or `None` when it's over and the frames should stop. A long pause between frames is taken as a short one, so a stalled frame doesn't look like a jump.
    pub fn step(&mut self, position: f64, dt: f64) -> Option<f64> {
        let motion = self.moving.as_mut()?;
        let mut left = dt.clamp(0.0, 0.05);
        let mut at = *motion.at.get_or_insert(position);
        // Whole steps of one size, and the rest carried in `left` by the last one, so the shape is the same at 60 or 120 frames a second.
        while left > 0.0 {
            let step = left.min(FOLLOW_STEP);
            left -= step;
            motion.aim += (motion.target - motion.aim) * (motion.rate * step).min(1.0);
            let acceleration = motion.rate * motion.rate * (motion.aim - at) - 2.0 * motion.rate * motion.velocity;
            motion.velocity += acceleration * step;
            at += motion.velocity * step;
        }
        motion.at = Some(at);
        if (at - motion.target).abs() < FOLLOW_ARRIVED.0 && motion.velocity.abs() < FOLLOW_ARRIVED.1 {
            let target = motion.target;
            self.moving = None;
            return Some(target);
        }
        Some(at)
    }
}
