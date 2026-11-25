use super::*;

/// The jobs the page view asks for now, each finished as done, up to the first for `page`, which is given back unfinished.
fn jobs_for(reader: &mut Reader, page: u32) -> Vec<Job> {
    let mut found = Vec::new();
    for _ in 0..200 {
        let Some(job) = reader.next_job() else { return found };
        if job.position == page && matches!(job.kind, JobKind::Draw { .. }) {
            found.push(job);
            return found;
        }
        match &job.kind {
            JobKind::Size => reader.finish(job, Outcome::Size(letter())),
            JobKind::Links => reader.finish(job, Outcome::Links(None)),
            JobKind::ImageBoxes { .. } => reader.finish(job, Outcome::ImageUnder(crate::image_pick::Under::No)),
            JobKind::Traits => reader.finish(job, Outcome::Heavy(false)),
            _ => reader.finish(
                job,
                Outcome::Drawn {
                    kind: TileKind::Colour,
                    low_res: None,
                },
            ),
        }
    }
    panic!("the page view kept asking");
}

/// Failure (second review of 2026-11-14, S1): on dark paper a tile whose page's look is to be tried again (`engine/look.rs`, `Outcome::LookLater`) is handed to the host and kept by a look that isn't the page's, a dark slide shown light; or it's asked for again as the very next job, so the look's tries go on tiles drawn and thrown away in milliseconds; or the person is told the page has a problem, or `settled` says it's done. Nothing of it is attached or kept, no note is shown, the page isn't settled and `qa state` counts it held, and nothing's asked of the page until the page view's wait is over: then the same tile is asked for again. The clock is moved by bringing the page's time forward. Seen failing with the hold ended at once (the tile asked for again as the next job).
#[test]
fn a_tile_whose_look_is_tried_again_is_held_quietly_till_its_time() {
    let mut reader = reader_on(10);
    reader.set_paper(PaperKind::Dark);
    let job = a_draw_job(&mut reader, DrawKind::Tile);
    let (page, rect, scale) = match &job.kind {
        JobKind::Draw { rect, scale, .. } => (job.position, *rect, *scale),
        _ => unreachable!(),
    };
    reader.work.note_started(&job, None);
    let (told, kept) = (reader.host.ops.len(), reader.cache.entries.len());
    reader.finish(job, Outcome::LookLater);
    assert!(
        !reader
            .host
            .ops
            .iter()
            .skip(told)
            .any(|op| matches!(op, Op::Attach { page: on, .. } if *on == page)),
        "nothing handed to the host: {:?}",
        reader.host.ops
    );
    assert_eq!(reader.cache.entries.len(), kept, "nothing kept");
    let held = jobs_for(&mut reader, page);
    assert!(held.is_empty(), "asked for while waiting: {held:?}");
    assert_eq!(reader.note(), None, "nothing said to the person");
    let report = reader.report().0;
    assert_eq!((report["settled"].as_bool(), report["looks_held"].as_u64()), (Some(false), Some(1)), "{report}");
    let issue = reader.docs.views.get_mut(&1).unwrap().drawing.issues.get_mut(&page).expect("held");
    let at = issue.try_again_at.expect("a time to try again");
    assert!(
        at >= Instant::now() + TRY_AGAIN_AFTER - Duration::from_millis(500),
        "held for the page view's wait"
    );
    issue.try_again_at = Some(Instant::now() - Duration::from_millis(1));
    reader.refresh(MAX_ATTACH_PER_TURN);
    let again = jobs_for(&mut reader, page);
    assert!(
        again
            .iter()
            .any(|job| matches!(&job.kind, JobKind::Draw { rect: asked, scale: at, .. } if *asked == rect && *at == scale)),
        "the same tile, asked for again at its time: {again:?}"
    );
}

/// Failure (the long session, 2026-11-25): a draw the Mac was too busy to run the worker for ends as `LookLater` on white paper too (`engine/run.rs`, `draw_failed`), and the page, which has no words to show, stays bare for the session: counted settled, never asked again. It isn't settled while it waits, and its tile is asked for again once the wait is over.
#[test]
fn a_busy_draw_on_white_paper_isnt_settled_and_is_asked_for_again() {
    let mut reader = reader_on(10);
    let job = a_draw_job(&mut reader, DrawKind::Tile);
    let (page, rect) = match &job.kind {
        JobKind::Draw { rect, .. } => (job.position, *rect),
        _ => unreachable!(),
    };
    reader.work.note_started(&job, None);
    assert_eq!(reader.docs.next_hold_ends(Instant::now()), None, "no page held, so no timer at rest");
    reader.finish(job, Outcome::LookLater);
    assert_eq!(reader.note(), None, "no words for a person");
    assert_eq!(reader.report().0["settled"].as_bool(), Some(false));
    assert!(
        reader
            .docs
            .next_hold_ends(Instant::now())
            .is_some_and(|left| !left.is_zero() && left <= TRY_AGAIN_AFTER),
        "the page view's thread is woken when the wait is over, as no event may come to ask (long session, 2026-11-25: the page stayed bare and unsettled for 30 s)"
    );
    assert!(jobs_for(&mut reader, page).is_empty(), "not asked for while it waits");
    reader.docs.views.get_mut(&1).unwrap().drawing.issues.get_mut(&page).expect("held").try_again_at = Some(Instant::now() - Duration::from_millis(1));
    // A wait that's over is not "nothing to wake for": the plan takes it, and the thread goes round at once to do so (see the test of that, below).
    reader.refresh(MAX_ATTACH_PER_TURN);
    let again = jobs_for(&mut reader, page);
    assert!(
        again.iter().any(|job| matches!(&job.kind, JobKind::Draw { rect: asked, .. } if *asked == rect)),
        "asked for again at its time: {again:?}"
    );
}

/// The first job of the kind `wanted` says, the others finished as done.
fn a_job_of(reader: &mut Reader, wanted: impl Fn(&JobKind) -> bool) -> Job {
    loop {
        let job = reader.next_job().expect("a job");
        if wanted(&job.kind) {
            return job;
        }
        match &job.kind {
            JobKind::Size => reader.finish(job, Outcome::Size(letter())),
            JobKind::Links => reader.finish(job, Outcome::Links(None)),
            JobKind::ImageBoxes { .. } => reader.finish(job, Outcome::ImageUnder(crate::image_pick::Under::No)),
            JobKind::Traits => reader.finish(job, Outcome::Heavy(false)),
            _ => reader.finish(
                job,
                Outcome::Drawn {
                    kind: TileKind::Colour,
                    low_res: None,
                },
            ),
        }
    }
}

/// Failure (the long session of 2026-11-25): a busy kill's hold outlived its retry. The page was drawn at its time and attached, but the hold, which nothing cleared, kept `settled` false for good: the thread at rest with no wake, `wait-settled` hung, a window shown for the tab waited out its whole patience. A draw's and a traits read's hold, on white and dark paper, are settled, and not counted held, once their time is over and the page is drawn again.
#[test]
fn a_held_page_retried_at_its_time_settles() {
    for paper in [PaperKind::White, PaperKind::Dark] {
        for traits in [false, true] {
            let mut reader = reader_on(10);
            reader.set_paper(paper);
            let job = if traits {
                a_job_of(&mut reader, |kind| matches!(kind, JobKind::Traits))
            } else {
                a_draw_job(&mut reader, DrawKind::Tile)
            };
            let page = job.position;
            reader.work.note_started(&job, None);
            reader.finish(job, Outcome::LookLater);
            let issue = reader.docs.views.get_mut(&1).unwrap().drawing.issues.get_mut(&page).expect("held");
            issue.try_again_at = Some(Instant::now() - Duration::from_millis(1));
            reader.refresh(MAX_ATTACH_PER_TURN);
            settle(&mut reader);
            let report = reader.report().0;
            assert!(
                reader.is_settled() && report["looks_held"].as_u64() == Some(0),
                "{paper:?}, traits job {traits}: settled {}, report {report}",
                reader.is_settled()
            );
        }
    }
}

/// Failure (the long session of 2026-11-25): the hold's time was over by the time the thread came to rest (the plan is made first, the rest after the lock is let go and the workers are seen to), so nothing was left to wake it and it parked for good with a page undrawn. A hold that is over says "go round now"; the next plan takes it, and then there's nothing to wake for.
#[test]
fn a_hold_over_before_the_rest_wakes_the_thread_at_once() {
    let mut reader = reader_on(10);
    let job = a_draw_job(&mut reader, DrawKind::Tile);
    let page = job.position;
    reader.work.note_started(&job, None);
    reader.finish(job, Outcome::LookLater);
    reader.docs.views.get_mut(&1).unwrap().drawing.issues.get_mut(&page).expect("held").try_again_at = Some(Instant::now() - Duration::from_millis(1));
    assert_eq!(
        reader.docs.next_hold_ends(Instant::now()),
        Some(Duration::ZERO),
        "a hold that is over, not yet taken by a plan, wakes the thread now"
    );
    let plan = reader.next_plan().expect("the held page is planned again");
    assert_eq!(plan.position, page, "the page that was held");
    assert_eq!(reader.docs.next_hold_ends(Instant::now()), None, "taken by the plan: nothing to wake for");
}

/// Failure (found with the long session of 2026-11-25, same root): a page that failed with words but may pass (`NotDrawableNow`) kept its issue after its retry drew, so the person was told, while the page showed, that it couldn't be drawn, and the page never settled. Once its time is over and it's drawn, the words go and the page is settled with its tiles on.
#[test]
fn a_page_that_may_pass_loses_its_words_once_its_retry_draws() {
    let mut reader = reader_on(10);
    let job = a_draw_job(&mut reader, DrawKind::Tile);
    let page = job.position;
    reader.work.note_started(&job, None);
    reader.finish(
        job,
        Outcome::PageFailed {
            message: "Page 1 can't be drawn just now.".into(),
            may_pass: true,
        },
    );
    assert!(reader.note().is_some(), "told while it waits");
    reader.docs.views.get_mut(&1).unwrap().drawing.issues.get_mut(&page).expect("held").try_again_at = Some(Instant::now() - Duration::from_millis(1));
    reader.refresh(MAX_ATTACH_PER_TURN);
    settle(&mut reader);
    assert_eq!(reader.note(), None, "drawn, so nothing to tell");
    assert!(reader.is_settled(), "settled: {}", reader.report().0);
    assert!(
        reader.cache.attached.get(&page).is_some_and(|ids| !ids.is_empty()),
        "the page's tiles are attached"
    );
}
