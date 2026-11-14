#![cfg(test)]

use super::*;
use recto_render_service::{KillReason, LowResWhy, Tile, TileKind};
use std::collections::VecDeque;
use std::time::Duration;

/// A renderer for one letter page, black all over, as a dark slide is, with images or none as a test says, whose small pictures come out as a test lists them, and drawn black once the list is done; or, with `fails_until`, fail for a reason that may pass until the test's clock, `now`, gets there.
struct StandIn {
    boxes: Option<Vec<[u16; 4]>>,
    draws: VecDeque<Result<(), RenderFailure>>,
    drawn: usize,
    now: Instant,
    fails_until: Option<Instant>,
}

impl StandIn {
    fn new(boxes: Option<Vec<[u16; 4]>>, draws: impl IntoIterator<Item = Result<(), RenderFailure>>) -> Self {
        Self {
            boxes,
            draws: draws.into_iter().collect(),
            drawn: 0,
            now: Instant::now(),
            fails_until: None,
        }
    }

    /// A page with no images whose small picture fails for a reason that may pass until `after` has gone by on the test's clock, which starts at `start`.
    fn failing_for(start: Instant, after: Option<Duration>) -> Self {
        Self {
            now: start,
            fails_until: Some(after.map_or(start + Duration::from_secs(86_400), |after| start + after)),
            ..Self::new(Some(Vec::new()), [])
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
        if self.fails_until.is_some_and(|until| self.now < until) {
            not_drawable_now()?;
        }
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

/// The page's look, as a tile drawn for part of it asks, with no picture of the whole page at hand, `after` into the test's clock from `start`.
fn asked_at(doc: &mut StandIn, looks: &mut Looks, start: Instant, after: Duration) -> Result<Arc<PageLook>, NoLook> {
    doc.now = start + after;
    page_look(doc, looks, 0, 1, None, None, doc.now)
}

/// The same, asked now.
fn asked(doc: &mut StandIn, looks: &mut Looks) -> Result<Arc<PageLook>, NoLook> {
    page_look(doc, looks, 0, 1, None, None, Instant::now())
}

fn seconds(tenths: u64) -> Duration {
    Duration::from_millis(tenths * 100)
}

/// Failure (second review of 2026-11-14, S1): on a page with no images, the small picture its look is decided from fails for a reason that may pass, a surface that can't be made under memory pressure, say, for 4 s, and the three tries are used up in milliseconds, three tiles drawn one after another and thrown away, so the page takes a page of text's look and keeps it: a dark slide turned light for the renderer's life. Tries are spaced by the page view's wait: asked every tenth of a second, the page is tried at 0, 3 and 6 s, nothing's drawn between, and at 6 s, the failure gone, it's decided dark and kept. Seen failing with every ask counted as a try (undecided, a page of text, at the third ask).
#[test]
fn a_small_picture_that_fails_for_a_few_seconds_is_tried_again_spaced_and_decided_then() {
    let start = Instant::now();
    let (mut doc, mut looks) = (StandIn::failing_for(start, Some(Duration::from_secs(4))), Looks::default());
    let mut tried_at = Vec::new();
    for tenth in 0..60 {
        let drawn = doc.drawn;
        assert_eq!(asked_at(&mut doc, &mut looks, start, seconds(tenth)), Err(NoLook::Again), "at {tenth} tenths");
        if doc.drawn > drawn {
            tried_at.push(tenth);
        }
        assert!(doc.drawn - drawn <= 1, "drawn more than once at {tenth} tenths");
        assert!(looks.decided.is_empty(), "nothing kept at {tenth} tenths: {looks:?}");
    }
    assert_eq!(tried_at, [0, 30], "tried once at each 3 s");
    assert_eq!(*asked_at(&mut doc, &mut looks, start, seconds(60)).unwrap(), PageLook::dark_page(), "decided at 6 s");
    assert_eq!(*asked_at(&mut doc, &mut looks, start, seconds(61)).unwrap(), PageLook::dark_page());
    assert_eq!(doc.drawn, 3, "a draw a try, and none once decided");
    assert!(looks.tries.is_empty(), "its tries go once it's decided: {looks:?}");
}

/// Failure: a page whose small picture can never be had for a reason that may pass (a Mac short of memory for as long as it's looked at) is tried again for ever, each try a tile drawn and thrown away, and is never shown; or its tries go in a moment. Asked every tenth of a second, it's tried at 0, 3 and 6 s and then takes what a page whose picture can't be had takes, and keeps it: a page of text's look with no images, dimmed with images. Seen failing with every ask counted as a try (undecided at the third ask, a fifth of a second in).
#[test]
fn a_page_whose_small_picture_keeps_failing_still_draws_after_three_spaced_tries() {
    for (boxes, expected) in [(Some(Vec::new()), PageLook::text()), (Some(vec![[0, 0, 32_768, 32_768]]), PageLook::unknown())] {
        let start = Instant::now();
        let mut doc = StandIn {
            boxes: boxes.clone(),
            ..StandIn::failing_for(start, None)
        };
        let mut looks = Looks::default();
        for tenth in 0..60 {
            assert_eq!(asked_at(&mut doc, &mut looks, start, seconds(tenth)), Err(NoLook::Again), "{boxes:?} at {tenth} tenths");
        }
        assert_eq!(doc.drawn, 2, "{boxes:?}: two tries in the first 6 s");
        assert_eq!(*asked_at(&mut doc, &mut looks, start, seconds(60)).unwrap(), expected, "{boxes:?}");
        assert_eq!(*asked_at(&mut doc, &mut looks, start, seconds(70)).unwrap(), expected, "{boxes:?}");
        assert_eq!(doc.drawn, usize::from(LOOK_TRIES), "kept: {boxes:?}");
    }
}

/// Failure: the step that maps a look to what a tile drawn for dark paper does hands a tile on, or keeps a look, by an answer that isn't the page's: a look to be tried again shown as undecided, a dark slide light. A look to be tried again ends the job as [`Outcome::LookLater`], which holds the page in the page view, with nothing kept; asked again before its time it ends the same way with nothing drawn; a look decided is gone by. Seen failing with `NoLook::Again` mapped to a shown, undecided look.
#[test]
fn a_look_to_be_tried_again_is_never_handed_on_or_kept() {
    let start = Instant::now();
    let (mut doc, mut looks) = (StandIn::failing_for(start, Some(Duration::from_secs(1))), Looks::default());
    let at = |doc: &mut StandIn, looks: &mut Looks, tenths| {
        doc.now = start + seconds(tenths);
        tile_look(doc, looks, 0, 1, (None, None), doc.now)
    };
    assert!(matches!(at(&mut doc, &mut looks, 0), Err(Outcome::LookLater)), "the try that failed");
    assert!(matches!(at(&mut doc, &mut looks, 10), Err(Outcome::LookLater)), "asked before its time");
    assert!(looks.decided.is_empty(), "nothing kept: {looks:?}");
    assert_eq!(doc.drawn, 1, "nothing drawn before its time");
    assert_eq!(*at(&mut doc, &mut looks, 30).unwrap(), PageLook::dark_page(), "decided at its time");
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
        *page_look(&mut doc, &mut looks, 0, 1, Some((&preview, rect)), None, Instant::now()).unwrap(),
        PageLook::dark_page()
    );
    assert_eq!(doc.drawn, 0);
}
