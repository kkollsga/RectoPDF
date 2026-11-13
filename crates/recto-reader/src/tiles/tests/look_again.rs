use super::*;

/// Failure (review of 2026-11-13, S2): on dark paper a tile whose page's look is to be tried again, its small picture having failed for a reason that may pass (`engine/look.rs`, `NoLook::Again`), is handed to the host and kept by a look that isn't the page's, a dark slide shown light; or it's never asked for again, so the page stays bare. The thread that draws ends such a job as cancelled: nothing of it is attached or kept, what's shown of the page stays, and the same tile is the next job asked for, at whose drawing the look is tried again.
#[test]
fn a_tile_whose_look_is_tried_again_is_not_shown_and_is_asked_for_again() {
    let mut reader = reader_on(10);
    reader.set_paper(PaperKind::Dark);
    let job = a_draw_job(&mut reader, DrawKind::Tile);
    let (page, rect, scale) = match &job.kind {
        JobKind::Draw { rect, scale, .. } => (job.position, *rect, *scale),
        _ => unreachable!(),
    };
    reader.work.note_started(&job, None);
    let (told, kept) = (reader.host.ops.len(), reader.cache.entries.len());
    reader.finish(job, Outcome::Cancelled);
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
    let again = a_draw_job(&mut reader, DrawKind::Tile);
    assert!(
        matches!(&again.kind, JobKind::Draw { rect: asked, scale: at, .. } if again.position == page && *asked == rect && *at == scale),
        "the same tile, asked for again: {again:?}"
    );
}
