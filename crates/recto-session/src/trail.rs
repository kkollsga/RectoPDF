//! **Back and forward,** a capped trail of places per tab, as a browser keeps one.
//!
//! A jump leaves a mark: following a link, choosing a contents entry, going to a page by number or label, as a person or an agent. Scrolling doesn't, as it's reading, not going somewhere. Back goes to the last mark and puts where the reader was on the way forward; a new jump clears the way forward, as on any Mac. At most [`MAX_TRAIL`] each way, the oldest going first, so a tab's trail costs at most a few hundred bytes, and a tab nobody's jumped in costs nothing. It's in the state record, so it comes back after a quit and across an update.

use serde::{Deserialize, Serialize};

/// Most places kept each way.
pub const MAX_TRAIL: usize = 50;

/// A place in a tab: a page, zero-based, and how far down it, from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stop {
    pub page: u32,
    pub within: f32,
}

impl Stop {
    pub fn new(page: u32, within: f64) -> Self {
        let within = if within.is_finite() { within.clamp(0.0, 1.0) as f32 } else { 0.0 };
        Self { page, within }
    }

    /// Close enough to be the same place: the same page and within a few lines.
    fn same_place(self, other: Self) -> bool {
        self.page == other.page && (self.within - other.within).abs() < 0.02
    }
}

/// One tab's trail.
// size: one Vec each way, held only for a tab someone has jumped in. At most `MAX_TRAIL` stops each way, 8 B a stop.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Trail {
    back: Vec<Stop>,
    forward: Vec<Stop>,
}

impl Trail {
    /// From what a record kept, which can't be believed: held to the cap, and to the pages there are.
    pub fn from_record(back: &[Stop], forward: &[Stop], pages: u32) -> Self {
        let keep = |stops: &[Stop]| -> Vec<Stop> {
            let start = stops.len().saturating_sub(MAX_TRAIL);
            stops[start..].iter().map(|stop| Stop::new(stop.page.min(pages.saturating_sub(1)), f64::from(stop.within))).collect()
        };
        Self { back: keep(back), forward: keep(forward) }
    }

    pub fn back(&self) -> &[Stop] {
        &self.back
    }

    pub fn forward(&self) -> &[Stop] {
        &self.forward
    }

    pub fn is_empty(&self) -> bool {
        self.back.is_empty() && self.forward.is_empty()
    }

    /// A jump from `here`: it's marked to come back to, and the way forward goes. A jump to where the reader already is marks nothing.
    pub fn jumped(&mut self, here: Stop, to: Stop) {
        if here.same_place(to) {
            return;
        }
        if !self.back.last().is_some_and(|last| last.same_place(here)) {
            self.back.push(here);
            if self.back.len() > MAX_TRAIL {
                self.back.remove(0);
            }
        }
        self.forward.clear();
    }

    /// Back from `here`, if there's somewhere to go: where to, with `here` kept for going forward again.
    pub fn go_back(&mut self, here: Stop) -> Option<Stop> {
        let to = self.back.pop()?;
        self.forward.push(here);
        if self.forward.len() > MAX_TRAIL {
            self.forward.remove(0);
        }
        Some(to)
    }

    /// Forward from `here`, the same way.
    pub fn go_forward(&mut self, here: Stop) -> Option<Stop> {
        let to = self.forward.pop()?;
        self.back.push(here);
        if self.back.len() > MAX_TRAIL {
            self.back.remove(0);
        }
        Some(to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: back doesn't return where the reader was, forward is lost before a new jump clears it, or a jump to the same place adds a step that back has to go through for nothing.
    #[test]
    fn back_and_forward_go_as_a_browser_does() {
        let mut trail = Trail::default();
        let at = |page| Stop::new(page, 0.25);
        trail.jumped(at(0), at(8));
        trail.jumped(at(8), at(3));
        trail.jumped(at(3), at(3));
        assert_eq!(trail.back(), [at(0), at(8)]);
        assert_eq!(trail.go_back(at(3)), Some(at(8)));
        assert_eq!(trail.go_back(at(8)), Some(at(0)));
        assert_eq!(trail.go_back(at(0)), None);
        assert_eq!(trail.go_forward(at(0)), Some(at(8)));
        trail.jumped(at(8), at(5));
        assert_eq!(trail.forward(), [], "a new jump clears the way forward");
        assert_eq!(trail.back(), [at(0), at(8)]);
    }

    /// Failure: a long session of links grows a tab's trail without end, or a record names pages past the end or more stops than the cap.
    #[test]
    fn the_trail_is_capped_both_ways() {
        let mut trail = Trail::default();
        for page in 0..(MAX_TRAIL as u32 * 3) {
            trail.jumped(Stop::new(page, 0.0), Stop::new(page + 1, 0.0));
        }
        assert_eq!(trail.back().len(), MAX_TRAIL);
        assert_eq!(trail.back()[0].page, MAX_TRAIL as u32 * 2, "the oldest went first");
        let many: Vec<Stop> = (0..500).map(|page| Stop { page, within: f32::NAN }).collect();
        let kept = Trail::from_record(&many, &many, 10);
        assert_eq!(kept.back().len(), MAX_TRAIL);
        assert!(kept.back().iter().chain(kept.forward()).all(|stop| stop.page < 10 && stop.within == 0.0));
    }
}
