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
    assert!(at >= Instant::now() + TRY_AGAIN_AFTER - Duration::from_millis(500), "held for the page view's wait");
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
