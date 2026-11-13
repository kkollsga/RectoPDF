#![cfg(test)]

use super::*;
use recto_render_service::{KillReason, LowResWhy, Tile, TileKind};
use std::collections::VecDeque;

/// A renderer for one letter page, black all over, as a dark slide is, with images or none as a test says, whose small pictures come out as a test lists them, and drawn black once the list is done.
struct StandIn {
    boxes: Option<Vec<[u16; 4]>>,
    draws: VecDeque<Result<(), RenderFailure>>,
    drawn: usize,
}

impl StandIn {
    fn new(boxes: Option<Vec<[u16; 4]>>, draws: impl IntoIterator<Item = Result<(), RenderFailure>>) -> Self {
        Self {
            boxes,
            draws: draws.into_iter().collect(),
            drawn: 0,
        }
    }
}

impl LookSource for StandIn {
    fn take_image_places_cleared(&mut self) -> bool {
        false
    }

    fn image_boxes(&mut self, _page: u32, _generation: u64) -> Result<Option<Vec<[u16; 4]>>, RenderFailure> {
        Ok(self.boxes.clone())
    }

    fn is_stale(&self, _generation: u64) -> bool {
        false
    }

    fn page_size(&mut self, _page: u32) -> Result<(f32, f32), RenderFailure> {
        Ok((612.0, 792.0))
    }

    fn render_tile(&mut self, _request: &TileRequest, surface: &PixelSurface) -> Result<TileResult, RenderFailure> {
        self.drawn += 1;
        self.draws.pop_front().unwrap_or(Ok(()))?;
        surface
            .with_pixels(|pixels| pixels.as_chunks_mut::<4>().0.iter_mut().for_each(|pixel| *pixel = [0, 0, 0, 0xFF]))
            .expect("a surface of the test's own");
        Ok(TileResult::Done(Tile {
            kind: TileKind::Grey,
            render_us: 0,
            low_res: None,
            region_kinds: Vec::new(),
        }))
    }
}

/// The failure of a draw that may pass: the larger limit busy, the lower resolution not fitting.
fn not_drawable_now() -> Result<(), RenderFailure> {
    Err(RenderFailure::NotDrawableNow {
        page: 0,
        why: LowResWhy::HeavyBusy,
    })
}

/// The page's look, as a tile drawn for part of it asks, with no picture of the whole page at hand.
fn asked(doc: &mut StandIn, looks: &mut Looks) -> Result<Arc<PageLook>, NoLook> {
    page_look(doc, looks, 0, 1, None, None)
}

/// Failure (review of 2026-11-13, S2): on a page with no images, the small picture its look is decided from fails for a reason that may pass (the larger limit busy for a moment), and the page takes a page of text's look and keeps it, so a dark slide is turned over, and stays light, for as long as its renderer lives. It's tried again: nothing's kept, the job's told to try again, and at the next job the page is decided from its picture, dark, and that's kept, so the job after draws nothing more. Seen failing with a passing failure kept as a final one.
#[test]
fn a_small_picture_that_fails_for_a_moment_is_tried_again_and_the_page_decided_then() {
    let (mut doc, mut looks) = (StandIn::new(Some(Vec::new()), [not_drawable_now()]), Looks::default());
    assert_eq!(asked(&mut doc, &mut looks), Err(NoLook::Again));
    assert!(looks.decided.is_empty(), "nothing kept: {looks:?}");
    assert_eq!(*asked(&mut doc, &mut looks).unwrap(), PageLook::dark_page(), "decided at the next job");
    assert_eq!(*asked(&mut doc, &mut looks).unwrap(), PageLook::dark_page());
    assert_eq!(doc.drawn, 2, "kept once decided");
    assert!(looks.tries.is_empty(), "its tries go once it's decided: {looks:?}");
}

/// Failure: a page whose small picture can never be had for a reason that may pass (a Mac short of memory for as long as it's looked at) is tried again for ever, each try a tile drawn and thrown away, and is never shown. After [`LOOK_TRIES`] it takes what a page whose picture can't be had takes, and keeps it: a page of text's look with no images, dimmed with images.
#[test]
fn a_page_whose_small_picture_keeps_failing_still_draws() {
    for (boxes, expected) in [(Some(Vec::new()), PageLook::text()), (Some(vec![[0, 0, 32_768, 32_768]]), PageLook::unknown())] {
        let mut doc = StandIn::new(boxes.clone(), std::iter::repeat_with(not_drawable_now).take(10));
        let mut looks = Looks::default();
        for _ in 1..LOOK_TRIES {
            assert_eq!(asked(&mut doc, &mut looks), Err(NoLook::Again), "{boxes:?}");
        }
        assert_eq!(*asked(&mut doc, &mut looks).unwrap(), expected, "{boxes:?}");
        assert_eq!(*asked(&mut doc, &mut looks).unwrap(), expected, "{boxes:?}");
        assert_eq!(doc.drawn, usize::from(LOOK_TRIES), "kept: {boxes:?}");
    }
}

/// Failure: a picture that can't be had for good (a page that stopped its worker) is tried again at every job, a tile thrown away each time. It takes [`PageLook::undecided`] at once and keeps it, as before.
#[test]
fn a_small_picture_that_fails_for_good_is_answered_at_once() {
    let failed = Err(RenderFailure::PageFailed {
        page: 0,
        reason: KillReason::Crashed,
    });
    let (mut doc, mut looks) = (StandIn::new(Some(Vec::new()), [failed]), Looks::default());
    assert_eq!(*asked(&mut doc, &mut looks).unwrap(), PageLook::text());
    assert_eq!(*asked(&mut doc, &mut looks).unwrap(), PageLook::text());
    assert_eq!(doc.drawn, 1);
}

/// Failure: a whole-page picture at hand, the preview this job drew, isn't used, and a small picture is drawn as well. The page is decided from it with nothing drawn.
#[test]
fn a_picture_of_the_whole_page_at_hand_decides_with_nothing_drawn() {
    let (mut doc, mut looks) = (StandIn::new(Some(Vec::new()), []), Looks::default());
    let preview = PixelSurface::create(153, 198).unwrap();
    preview
        .with_pixels(|pixels| pixels.as_chunks_mut::<4>().0.iter_mut().for_each(|pixel| *pixel = [0, 0, 0, 0xFF]))
        .unwrap();
    let rect = TileRect {
        x: 0,
        y: 0,
        width: 153,
        height: 198,
    };
    assert_eq!(
        *page_look(&mut doc, &mut looks, 0, 1, Some((&preview, rect)), None).unwrap(),
        PageLook::dark_page()
    );
    assert_eq!(doc.drawn, 0);
}
