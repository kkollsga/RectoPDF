#![cfg(test)]

use super::*;
use recto_render_service::{KillReason, LowResWhy, Tile, TileKind};
use std::collections::VecDeque;
use std::time::Duration;

/// A renderer for one letter page, black all over, as a dark slide is, with images or none as a test says, whose small pictures come out as a test lists them, and drawn black once the list is done; or, with `fails_until`, fail for a reason that may pass until the test's clock, `now`, gets there.
struct StandIn {
    boxes: Option<Vec<[u16; 4]>>,
    /// Its next image checks the Mac was too busy for, as many as are left.
    busy_images: u8,
    draws: VecDeque<Result<(), RenderFailure>>,
    drawn: usize,
    now: Instant,
    fails_until: Option<Instant>,
    /// What its small pictures are drawn in, black but where a test says.
    colour: [u8; 4],
    /// What its page size read fails with, if it does.
    size_fails: Option<RenderFailure>,
}

impl StandIn {
    fn new(boxes: Option<Vec<[u16; 4]>>, draws: impl IntoIterator<Item = Result<(), RenderFailure>>) -> Self {
        Self {
            boxes,
            busy_images: 0,
            draws: draws.into_iter().collect(),
            drawn: 0,
            now: Instant::now(),
            fails_until: None,
            colour: [0, 0, 0, 0xFF],
            size_fails: None,
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

    fn image_boxes(&mut self, _page: u32, _generation: u64) -> ImagesAsked {
        if self.busy_images > 0 {
            self.busy_images -= 1;
            return ImagesAsked::Busy;
        }
        self.boxes.clone().map_or(ImagesAsked::Unknown, ImagesAsked::Known)
    }

    fn is_stale(&self, _generation: u64) -> bool {
        false
    }

    fn page_size(&mut self, _page: u32) -> Result<(f32, f32), RenderFailure> {
        if let Some(failure) = self.size_fails {
            return Err(failure);
        }
        Ok((612.0, 792.0))
    }

    fn render_tile(&mut self, _request: &TileRequest, surface: &PixelSurface) -> Result<TileResult, RenderFailure> {
        self.drawn += 1;
        if self.fails_until.is_some_and(|until| self.now < until) {
            not_drawable_now()?;
        }
        self.draws.pop_front().unwrap_or(Ok(()))?;
        let colour = self.colour;
        surface
            .with_pixels(|pixels| pixels.as_chunks_mut::<4>().0.iter_mut().for_each(|pixel| *pixel = colour))
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
    assert_eq!(
        *asked_at(&mut doc, &mut looks, start, seconds(60)).unwrap(),
        PageLook::dark_page(),
        "decided at 6 s"
    );
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
            assert_eq!(
                asked_at(&mut doc, &mut looks, start, seconds(tenth)),
                Err(NoLook::Again),
                "{boxes:?} at {tenth} tenths"
            );
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
        tile_look(doc, looks, 0, 1, None, None, doc.now)
    };
    assert!(matches!(at(&mut doc, &mut looks, 0), Err(Outcome::LookLater)), "the try that failed");
    assert!(matches!(at(&mut doc, &mut looks, 10), Err(Outcome::LookLater)), "asked before its time");
    assert!(looks.decided.is_empty(), "nothing kept: {looks:?}");
    assert_eq!(doc.drawn, 1, "nothing drawn before its time");
    assert_eq!(*at(&mut doc, &mut looks, 30).unwrap(), PageLook::dark_page(), "decided at its time");
}

/// Failure (review of the long-session wake, B1): the small picture a dark page's look is decided from comes back `TooBusy` when the Mac gave the worker no processor, and the look took it for a final failure: a dark slide's look was decided and kept as a page of text, shown light for the renderer's life. It's a try that may pass: nothing's kept and the page is asked again after the wait. Seen failing with `TooBusy` as `Picture::Final` (decided at the first ask).
#[test]
fn a_small_picture_the_mac_was_too_busy_for_is_tried_again_not_decided() {
    let (mut doc, mut looks) = (StandIn::new(Some(Vec::new()), [Err(RenderFailure::TooBusy { page: 0 })]), Looks::default());
    assert_eq!(asked(&mut doc, &mut looks), Err(NoLook::Again));
    assert!(looks.decided.is_empty(), "nothing kept: {looks:?}");
}

/// Failure (final review): the page's size, asked first for the small picture, comes back `TooBusy` and the look took it for a final failure, as the draw's was before B1. It's a try that may pass. Seen failing with that `Picture::Final` (decided at the first ask).
#[test]
fn a_page_size_the_mac_was_too_busy_for_is_tried_again_not_decided() {
    let mut doc = StandIn::new(Some(Vec::new()), []);
    doc.size_fails = Some(RenderFailure::TooBusy { page: 0 });
    let mut looks = Looks::default();
    assert_eq!(asked(&mut doc, &mut looks), Err(NoLook::Again));
    assert!(looks.decided.is_empty(), "nothing kept: {looks:?}");
    assert_eq!(doc.drawn, 0);
}

/// Failure (the owner's 2026-11-13 report): the check for where a page's pictures are came back unknown because the Mac was too busy to run its worker, and the page was judged whole and kept so, a map sheet on white paper dimmed grey for the renderer's life. A busy check decides nothing and keeps nothing: the job ends as a look to be tried again, nothing is drawn for it, and once the wait is over and the Mac has the processor the look is decided from where the images are. Seen failing with `Busy` mapped to `Unknown` (the look kept at the first ask, dimmed).
#[test]
fn a_busy_image_check_makes_the_look_wait_and_it_is_decided_from_the_images_then() {
    let start = Instant::now();
    let picture = vec![[0, 0, 65_535, 65_535]];
    let mut doc = StandIn::new(Some(picture.clone()), []);
    doc.busy_images = 1;
    let mut looks = Looks::default();
    assert_eq!(asked_at(&mut doc, &mut looks, start, seconds(0)), Err(NoLook::Again));
    assert!(looks.decided.is_empty(), "nothing kept: {looks:?}");
    assert_eq!(doc.drawn, 0, "and nothing drawn for it");
    // Decided as the same page is on a Mac that was never busy.
    let mut quiet = StandIn::new(Some(picture), []);
    let expected = asked(&mut quiet, &mut Looks::default()).unwrap();
    assert_eq!(asked_at(&mut doc, &mut looks, start, seconds(30)).unwrap(), expected);
    assert_eq!(doc.drawn, 1, "from one picture, drawn once the wait was over");
}

/// Failure: a page whose worker hangs makes every image check busy, so a look that waits for ever is a page never shown. The tries are [`LOOK_TRIES`], spaced, after which the page is judged whole as one whose images can't be told, and kept.
#[test]
fn a_page_whose_image_check_stays_busy_is_judged_whole_after_the_tries() {
    let start = Instant::now();
    let mut doc = StandIn::new(Some(vec![[0, 0, 65_535, 65_535]]), []);
    doc.busy_images = u8::MAX;
    let mut looks = Looks::default();
    for tenth in [0, 30] {
        assert_eq!(asked_at(&mut doc, &mut looks, start, seconds(tenth)), Err(NoLook::Again), "at {tenth} tenths");
    }
    assert_eq!(*asked_at(&mut doc, &mut looks, start, seconds(60)).unwrap(), PageLook::unknown());
    assert!(looks.decided.contains_key(&0), "kept");
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

/// Failure (review of 2026-11-13, S4, and the second review of 2026-11-14): a page with no images is decided from a tile that isn't the whole of it, as when `covers_page` takes any tile for the page, so a white page whose dark band fills that tile is dimmed whole. The real-file test of it (`looks_on_real_files.rs`, `a_white_page_whose_band_fills_its_first_tile_turns_over`) catches it only while its top left tile is drawn first; this asks which picture decided, whatever the order: a dark tile of part of a white page decides nothing, and the page is decided from its small picture, white, with one draw; a dark tile that's the whole page decides it, with none. Seen failing with `covers_page` always saying yes.
#[test]
fn only_a_tile_that_is_the_whole_page_decides_it() {
    let tile = PixelSurface::create(64, 64).unwrap();
    tile.with_pixels(|pixels| pixels.as_chunks_mut::<4>().0.iter_mut().for_each(|pixel| *pixel = [0, 0, 0, 0xFF]))
        .unwrap();
    let part = TileRect {
        x: 0,
        y: 0,
        width: 64,
        height: 64,
    };
    let mut doc = StandIn {
        colour: [0xFF, 0xFF, 0xFF, 0xFF],
        ..StandIn::new(Some(Vec::new()), [])
    };
    let mut looks = Looks::default();
    // At a scale of 1 a letter page is 612 by 792: the tile is its top left corner.
    assert_eq!(
        *page_look(&mut doc, &mut looks, 0, 1, None, Some((&tile, part, 1.0)), Instant::now()).unwrap(),
        PageLook::text(),
        "a corner of a white page"
    );
    assert_eq!(doc.drawn, 1, "decided from its small picture");
    // At 64 / 792 the whole page is 49 by 64.
    let (mut doc, mut looks) = (StandIn::new(Some(Vec::new()), []), Looks::default());
    let whole = TileRect {
        x: 0,
        y: 0,
        width: 49,
        height: 64,
    };
    let scale = 64.0 / 792.0;
    assert_eq!(
        recto_proto::device_size(612.0, 792.0, scale).ok(),
        Some((49, 64)),
        "the made page's size at that scale"
    );
    assert_eq!(
        *page_look(&mut doc, &mut looks, 0, 1, None, Some((&tile, whole, scale)), Instant::now()).unwrap(),
        PageLook::dark_page(),
        "a dark tile that's the whole page"
    );
    assert_eq!(doc.drawn, 0, "nothing drawn for it");
}
