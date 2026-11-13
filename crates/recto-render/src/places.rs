//! Where a page's images are, as the walk over its objects finds them. **Up to [`MAX_IMAGE_BOXES`] distinct places, each is given as it is,** and the decision judges each image by itself. **Past that, the page folds:** images whose boxes overlap or share an edge become one picture, the box round them, as a picture cut into strips or a map with its insets laid over it is, the boxes already held included. If more than [`MAX_IMAGE_BOXES`] pictures are still apart, there are too many to tell apart: the page is judged whole by its pixels (`SPEC.md`, dark paper), and the walk stops there, as nothing after that changes the answer.
//!
//! **So the rule has a step at 64.** A page of 64 images is judged image by image; the same page with one image more is judged by the boxes round the images that touch. A folded box is a bounding box and takes in any paper between its images, which can tip it into the white-backed test, so a photograph folded with a white-backed chart beside it can turn over with it. That's why folding waits for a page past 64, rather than folding every page: folded everywhere, the 1,291 surveyed pages had a 3D rendering on an arXiv cover and a coloured USGS map, each folded with the white round them, turn over where they'd been kept (2026-11-13).
//!
//! **Why not one box round them all, and why not refuse at 65 images.** Merged into one box round every image, a catalogue page of 72 photos on white came out one picture over most of a page that turned over, kept bright (second review of 2026-11-13). Refused at 65 images whatever they were, the USGS sheet `70277338`, a map with insets laid over it, 107 images, was judged whole and dimmed, where with its images known it turns over and keeps its map, as the owner was shown. Folded where they touch, the catalogue's photos, with gaps between them, stay 72 and are judged whole; the sheet's map and insets are a few pictures.
//!
//! **What touches.** Boxes that overlap, or whose edges meet or are at most [`TOUCH`] apart along a stretch of some length: strips of one picture abut exactly, give or take the rounding to 65535ths. Boxes that meet only at a corner don't touch: photos laid out on a diagonal share one point, and one point isn't one picture.
//!
//! **What it costs, whatever the page.** At most [`MAX_IMAGE_BOXES`] boxes are held at any moment. Up to the step, each place found is compared with each of them once. At the step the boxes held are folded, each taken out and put back as a place found past the step. Past it, a place is compared with each box held once, in a pass; a pass that folds one in is followed by another, as the box has grown and may now touch one it didn't, and a pass that folds none ends it. Every fold takes away a box that an earlier place put there, so there are no more folds than places, and over the walk no more than `2 × 64` comparisons a place on average, with the 64 held boxes counted again at the step, and the walk's own cap of 20,000 objects.

use recto_proto::MAX_IMAGE_BOXES;
use std::ops::ControlFlow;

/// How far apart, in 65535ths of the page, two edges may be and still meet: the rounding of each to 65535ths.
pub(crate) const TOUCH: u16 = 1;

/// The places found so far, `[left, top, right, bottom]` in 65535ths of the page: each image's, or past the step each the box round images that touch.
#[derive(Debug, Default)]
pub(crate) struct ImagePlaces {
    found: Vec<[u16; 4]>,
    /// Past the step: more distinct places than an answer holds were found, and the page folds.
    folding: bool,
    /// How many comparisons of two boxes, all told. See the module's notes for the bound.
    compared: usize,
}

/// Whether two boxes overlap or meet along an edge, as the module's notes say.
fn touch(one: [u16; 4], two: [u16; 4]) -> bool {
    // How far they run together across, and down: negative is a gap.
    let across = i32::from(one[2].min(two[2])) - i32::from(one[0].max(two[0]));
    let down = i32::from(one[3].min(two[3])) - i32::from(one[1].max(two[1]));
    let near = -i32::from(TOUCH);
    across >= near && down >= near && (across > 0 || down > 0)
}

impl ImagePlaces {
    /// Takes a place the walk found. `Break` once there are too many pictures apart to tell: the walk can stop.
    pub(crate) fn add(&mut self, place: [u16; 4]) -> ControlFlow<()> {
        if !self.folding {
            self.compared += self.found.len();
            if self.found.contains(&place) {
                return ControlFlow::Continue(());
            }
            if self.found.len() < MAX_IMAGE_BOXES {
                self.found.push(place);
                return ControlFlow::Continue(());
            }
            // The step: fold what's held, then this.
            self.folding = true;
            for held in std::mem::take(&mut self.found) {
                // Never more than were held, so never past what an answer holds.
                let _ = self.fold_in(held);
            }
        }
        self.fold_in(place)
    }

    /// Past the step: folds `place` in with every box it touches, and those its growing box comes to touch.
    fn fold_in(&mut self, place: [u16; 4]) -> ControlFlow<()> {
        let mut grown = place;
        loop {
            let mut folded = false;
            let mut at = 0;
            while at < self.found.len() {
                self.compared += 1;
                let other = self.found[at];
                if touch(grown, other) {
                    grown = [grown[0].min(other[0]), grown[1].min(other[1]), grown[2].max(other[2]), grown[3].max(other[3])];
                    self.found.swap_remove(at);
                    folded = true;
                } else {
                    at += 1;
                }
            }
            if !folded {
                break;
            }
        }
        if self.found.len() == MAX_IMAGE_BOXES {
            return ControlFlow::Break(());
        }
        self.found.push(grown);
        ControlFlow::Continue(())
    }

    pub(crate) fn into_places(self) -> Vec<[u16; 4]> {
        self.found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The places found for `boxes`, sorted, or `None` for too many, and the comparisons made.
    fn walk(boxes: impl IntoIterator<Item = [u16; 4]>) -> (Option<Vec<[u16; 4]>>, usize) {
        let mut places = ImagePlaces::default();
        for place in boxes {
            if places.add(place).is_break() {
                return (None, places.compared);
            }
        }
        let compared = places.compared;
        let mut found = places.into_places();
        found.sort_unstable();
        (Some(found), compared)
    }

    /// A square of `side` at `(x, y)`.
    fn square(x: u16, y: u16, side: u16) -> [u16; 4] {
        [x, y, x + side, y + side]
    }

    /// `count` small squares far apart, along the bottom of the page, to take a page to the step.
    fn apart(count: u16) -> impl Iterator<Item = [u16; 4]> {
        (0..count).map(|at| square((at % 16) * 4_000, 50_000 + (at / 16) * 3_000, 1_000))
    }

    /// Failure: a page of up to 64 images has them folded, so a photograph folded with the white round a chart beside it turns into a negative (2026-11-13: an arXiv cover's 3D rendering and a USGS map, over the survey); or a page past 64 isn't folded, and a map with insets laid over it, the USGS sheet `70277338`'s 107 images, is refused and dimmed whole. The same made page both sides of the step: 64 images, each overlapping the next, are given one by one; one image more, and they're one box round them all.
    #[test]
    fn a_page_folds_only_past_what_an_answer_holds() {
        let chain = |count: u16| (0..count).map(|at| square(at * 300, 1_000, 400));
        assert_eq!(walk(chain(64)).0.map(|found| found.len()), Some(64), "64: each as it is");
        assert_eq!(walk(chain(65)).0, Some(vec![[0, 1_000, 64 * 300 + 400, 1_400]]), "65: folded");
    }

    /// Failure: past the step, a picture cut into strips, or images whose boxes chain together, are left as more than one box, or a box that grows by a fold is left overlapping one it was compared with before. A chain of 100 boxes in an order that only joins up at the end, 200 strips sharing edges, and a box closing round one already passed over each come out one box.
    #[test]
    fn past_the_step_images_that_overlap_or_share_an_edge_are_one_picture() {
        // Even ones first, then the odd ones that join each pair: nothing joins until the odd ones come.
        let chain = (0..100u16).step_by(2).chain((1..100).step_by(2)).map(|at| square(at * 300, 1_000, 400));
        assert_eq!(walk(chain).0, Some(vec![[0, 1_000, 99 * 300 + 400, 1_400]]));
        let strips = (0..200u16).map(|at| [5_000, 10_000 + at * 100, 30_000, 10_100 + at * 100]);
        assert_eq!(walk(strips).0, Some(vec![[5_000, 10_000, 30_000, 30_000]]));
        // Past the step, which four boxes in a row and 61 apart take the page to, folding the four into one: a box, one right of it, and one below it, apart from it, until the third joins the second and the box round them takes in the first, already passed over.
        let row = (0..4u16).map(|at| square(10_000 + at * 300, 20_000, 400));
        let closing = [[0, 0, 1_000, 1_000], [2_000, 0, 3_000, 3_000], [0, 2_000, 2_500, 3_000]];
        let (found, _) = walk(row.chain(apart(61)).chain(closing));
        assert!(
            found.as_ref().is_some_and(|found| found.contains(&[0, 0, 3_000, 3_000]) && found.len() == 63),
            "{found:?}"
        );
    }

    /// Failure: past the step, boxes that only meet at a corner, photos laid out on a diagonal, are folded into one picture with the paper between them; or edges a rounding apart, strips of one picture, aren't. With 63 far-apart boxes and the two in question, 65: folded to 64 if the two touch, too many if they don't.
    #[test]
    fn past_the_step_edges_that_meet_touch_and_corners_do_not() {
        let with = |pair: [[u16; 4]; 2]| walk(apart(63).chain(pair)).0.map(|found| found.len());
        assert_eq!(with([square(0, 0, 100), square(100, 0, 100)]), Some(64), "edge to edge");
        assert_eq!(with([square(0, 0, 100), square(101, 0, 100)]), Some(64), "a rounding apart");
        assert_eq!(with([square(0, 0, 100), square(100, 100, 100)]), None, "corner to corner");
        assert_eq!(with([square(0, 0, 100), square(102, 0, 100)]), None, "a gap");
    }

    /// Failure: a catalogue page of photos with gaps between them, more than an answer holds, is answered with one box round them all, kept bright on a page that turns over; or a page with exactly as many apart is refused; or one image drawn twice in the same place counts twice. 64 apart are given; 65 apart are too many; 65 images in 64 places are 64.
    #[test]
    fn the_picture_past_what_an_answer_holds_stops_the_walk() {
        assert_eq!(walk(apart(64)).0.map(|found| found.len()), Some(MAX_IMAGE_BOXES));
        assert_eq!(walk(apart(65)).0, None);
        assert_eq!(walk(apart(64).chain(apart(1))).0.map(|found| found.len()), Some(MAX_IMAGE_BOXES));
    }

    /// Failure (rule 6): a hostile page of 10,000 tiny images scattered apart, or 20,000 in a chain that keeps folding, costs a comparison with every box found before, for each, as when every distinct place was kept: up to 200 million. Scattered, the walk stops at the first place past the step that can't fold, having compared each with at most 64 and the 64 held again at the step; folding, it makes no more than two passes' worth of comparisons a place over the walk, the step's included.
    #[test]
    fn the_walk_compares_a_bounded_number_of_boxes_a_place() {
        let scattered = (0..10_000u32).map(|at| {
            let (x, y) = ((at * 7_919) % 65_000, (at * 104_729 / 7) % 65_000);
            [x as u16, y as u16, x as u16 + 2, y as u16 + 2]
        });
        let mut places = ImagePlaces::default();
        let mut taken = 0;
        for place in scattered {
            taken += 1;
            if places.add(place).is_break() {
                break;
            }
        }
        assert!(taken <= 200, "stopped after {taken}");
        assert!(
            places.compared <= 2 * MAX_IMAGE_BOXES * (taken + MAX_IMAGE_BOXES),
            "{} comparisons for {taken}",
            places.compared
        );
        // 64 boxes apart, then a box from the left that grows to reach each in turn: past the step, each place folds one and the pass runs again.
        let held = (0..64u16).map(|at| square(at * 1_000, 0, 500));
        let reaching = (0..20_000u32).map(|at| [0, 0, (at % 64_000) as u16 + 500, 500]);
        let mut places = ImagePlaces::default();
        for place in held.chain(reaching) {
            assert!(places.add(place).is_continue());
        }
        assert!(places.found.len() <= MAX_IMAGE_BOXES);
        assert!(places.compared <= 2 * MAX_IMAGE_BOXES * (20_064 + MAX_IMAGE_BOXES), "{}", places.compared);
    }
}
