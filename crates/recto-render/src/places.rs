//! Where a page's images are, as the walk over its objects finds them: each distinct place once, up to [`MAX_IMAGE_BOXES`]. One more distinct place and there are too many to tell apart. The page is then judged whole by its pixels (`SPEC.md`, dark paper), and the walk stops there, as nothing after that changes the answer. Merged into one box round them all, a catalogue page of 72 photos on white came out as one picture over 90% of the page, kept bright on a page that turned over (second review of 2026-11-13).

use recto_proto::MAX_IMAGE_BOXES;
use std::ops::ControlFlow;

/// The distinct places found so far, `[left, top, right, bottom]` in 65535ths of the page.
#[derive(Debug, Default)]
pub(crate) struct ImagePlaces {
    found: Vec<[u16; 4]>,
    /// How many places a new one has been compared with, all told: at most [`MAX_IMAGE_BOXES`] for each place added, however many objects the page has.
    compared: usize,
}

impl ImagePlaces {
    /// Takes a place the walk found. `Break` once there are more distinct places than an answer holds: the walk can stop.
    pub(crate) fn add(&mut self, place: [u16; 4]) -> ControlFlow<()> {
        self.compared += self.found.len();
        if !self.found.contains(&place) {
            if self.found.len() == MAX_IMAGE_BOXES {
                return ControlFlow::Break(());
            }
            self.found.push(place);
        }
        ControlFlow::Continue(())
    }

    pub(crate) fn into_places(self) -> Vec<[u16; 4]> {
        self.found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(at: usize) -> [u16; 4] {
        let at = at as u16;
        [at, at, at + 1, at + 1]
    }

    /// Failure: a page with more images than an answer holds is answered with one box round them all, which on a page of photos on white is kept as one bright picture on a page that turns over; or a page with exactly as many as an answer holds, or with more objects than that drawing fewer distinct images, is called too many. Each distinct place counts once, the last one that fits is kept, and the one after it stops the walk.
    #[test]
    fn the_place_past_what_an_answer_holds_stops_the_walk() {
        let mut places = ImagePlaces::default();
        for at in 0..MAX_IMAGE_BOXES {
            assert_eq!(places.add(place(at)), ControlFlow::Continue(()), "{at}");
            assert_eq!(places.add(place(at)), ControlFlow::Continue(()), "{at} again");
        }
        assert_eq!(places.add(place(MAX_IMAGE_BOXES)), ControlFlow::Break(()));
        let mut places = ImagePlaces::default();
        for at in 0..MAX_IMAGE_BOXES {
            let _ = places.add(place(at));
        }
        assert_eq!(places.into_places(), (0..MAX_IMAGE_BOXES).map(place).collect::<Vec<_>>());
    }

    /// Failure (rule 6): a page of 20,000 images in distinct places, the most objects the walk looks at, costs each a comparison with every place found before it, 200 million in all, as it did when every distinct place was kept and only merged at the end. The walk stops at the first place past what an answer holds, having compared at most that many for each.
    #[test]
    fn the_walk_compares_each_object_with_at_most_what_an_answer_holds() {
        let objects = 20_000;
        let mut places = ImagePlaces::default();
        let mut stopped_at = None;
        for at in 0..objects {
            if places.add(place(at)).is_break() {
                stopped_at = Some(at);
                break;
            }
        }
        assert_eq!(stopped_at, Some(MAX_IMAGE_BOXES));
        assert!(places.compared <= MAX_IMAGE_BOXES * (MAX_IMAGE_BOXES + 1), "{}", places.compared);
    }
}
