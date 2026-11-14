//! What the host is given of highlights (`tiles/marks.rs`).

use super::*;
use crate::paper::PaperKind;
use crate::tiles::TabMarks;
use recto_index::select::Rect;

fn rect(x: f32, width: f32) -> Rect {
    Rect {
        x,
        y: 10.0,
        width,
        height: 10.0,
    }
}

/// Failure: a set made wrongly (a NaN, an infinite or negative edge, a rectangle past its page, a page the tab hasn't, or more than the caps)
/// reaches the host to draw. The caps held at themselves plus one, set small.
#[test]
fn what_the_host_is_given_is_held_to_what_can_be_drawn() {
    let mut reader = reader_on(3);
    settle(&mut reader);
    let tab = reader.docs.hot_tab().expect("a tab showing");
    let marks = TabMarks {
        tab,
        pages: vec![
            (
                0,
                vec![
                    rect(10.0, 50.0),
                    rect(f32::NAN, 5.0),
                    rect(-1.0, 5.0),
                    rect(10.0, f32::INFINITY),
                    rect(600.0, 100.0),
                ],
            ),
            (1, vec![rect(10.0, 5.0), rect(20.0, 5.0), rect(30.0, 5.0)]),
            (2, vec![rect(10.0, 5.0)]),
            (9, vec![rect(10.0, 5.0)]),
        ],
        hidden: None,
    };
    let reader = &mut reader;
    reader.host.take_ops();
    super::super::marks::tell_within(&mut reader.host, &reader.docs, &marks, PaperKind::White, (2, 3));
    let ops = reader.host.take_ops();
    let Some(Op::Marks { pages, .. }) = ops.last() else { panic!("{ops:?}") };
    let counts: Vec<(u32, usize)> = pages.iter().map(|(page, rects)| (*page, rects.len())).collect();
    assert_eq!(
        counts,
        vec![(0, 1), (1, 2)],
        "page 0's one sound line, page 1 to the rectangles' cap, and past the pages' cap nothing"
    );
    let other = TabMarks {
        tab: tab + 1,
        pages: marks.pages.clone(),
        hidden: None,
    };
    super::super::marks::tell_within(&mut reader.host, &reader.docs, &other, PaperKind::White, (2, 3));
    assert!(reader.host.take_ops().is_empty(), "another tab's set gives nothing");
}

/// **Failure (review of R4.3, S2): a tab's set of hidden file highlights is kept when it's emptied**, so the page view holds something for a tab with
/// none, or the map keeps its room; or a set given again draws pages again; or a changed one doesn't draw again the pages either set is on.
#[test]
fn a_hidden_set_is_held_only_while_it_hides_something() {
    use recto_proto::hidden::HiddenAnnots;
    use std::sync::Arc;
    let set = |pages: &[u32], generation: u64| {
        Some(Arc::new(HiddenAnnots {
            generation,
            pages: pages.iter().map(|page| (*page, vec![(0, [0.0; 4])])).collect(),
        }))
    };
    let mut sets = super::super::HiddenSets::new();
    let file = std::path::PathBuf::from("/d/a.pdf");
    assert_eq!(super::super::put_hidden(&mut sets, 1, file.clone(), set(&[2, 4], 1)), vec![2, 4]);
    assert!(
        super::super::put_hidden(&mut sets, 1, file.clone(), set(&[2, 4], 2)).is_empty(),
        "the same pages hidden: nothing to draw again"
    );
    assert_eq!(sets[&(1, file.clone())].generation, 1, "and the worker's set is kept");
    assert_eq!(super::super::put_hidden(&mut sets, 1, file.clone(), set(&[4, 5], 3)), vec![2, 4, 5]);
    assert_eq!(
        super::super::put_hidden(&mut sets, 1, file.clone(), set(&[], 4)),
        vec![4, 5],
        "an empty set is none"
    );
    assert!(sets.is_empty() && sets.capacity() == 0, "nothing held, not even the room");
    assert!(super::super::put_hidden(&mut sets, 1, file, None).is_empty());
}

/// **Failure (second review of R4.3, S4): a draw already running when a tab's set of hidden file highlights changes lands afterwards and is kept**,
/// so a tile drawn with the old set (a highlight the person took out, or one they just put back missing) stays on screen: the last thing asked for
/// loses to a read still landing. A draw held in flight across the change isn't kept; its page is asked for again.
#[test]
fn a_draw_in_flight_across_a_set_change_is_not_kept() {
    let mut reader = reader_on(3);
    let job = a_draw_job(&mut reader, DrawKind::Tile);
    reader.work.note_started(&job, None);
    let (tab, position) = (job.tab, job.position);
    let cancelled = reader.counts().cancelled;
    super::super::redraw_own_pages(&mut reader, tab, &[job.source_page]);
    reader.finish(
        job,
        Outcome::Drawn {
            kind: TileKind::Colour,
            low_res: None,
        },
    );
    assert_eq!(reader.counts().cancelled, cancelled + 1, "drawn under the old set, it isn't kept");
    let again = std::iter::from_fn(|| reader.next_job()).any(|job| job.position == position && matches!(job.kind, JobKind::Draw { .. }));
    assert!(again, "its page is drawn again, under the set as it is now");
}
