use super::*;
use crate::tiles::relook::{bare, drop_look, stand_in_plan};

/// A dark-paper reader on three pages with every picture drawn and attached, and a warm second tab on the same file with its own: the second tab's page 0 has tiles and a preview of the old look, as the first has.
fn two_tabs_on_one_file() -> Reader {
    let mut reader = Reader::new();
    reader.set_paper(PaperKind::Dark);
    reader.set_viewport(view(0.0));
    reader.activate(2, 3, 0, map(), &[], None, Spread::Single);
    reader.set_viewport(view(0.0));
    settle(&mut reader);
    reader.activate(1, 3, 0, map(), &[2], None, Spread::Single);
    reader.set_viewport(view(0.0));
    settle(&mut reader);
    reader
}

fn drawn_of(reader: &Reader, tab: u32, page: u32) -> Vec<u64> {
    reader
        .cache
        .entries
        .values()
        .filter(|entry| entry.tab == tab && entry.page == page)
        .map(|entry| entry.id)
        .collect()
}

/// Failure (a look kept for want of an answer, asked again and different): the new look's page is drawn over tiles of the old one, two looks at once; or the old tiles go before the new picture is there, one blank frame; or only the shown tab's go, and the warm tab on the same file wakes up showing the dimmed page for good. One call that drops the page's old pictures and finishes the new preview leaves page 0 with the new preview only, attached, in both tabs; the host is told the `Detach`es and the `Attach` in the same batch, the detaches first; page 1 is untouched, no step is counted blank, and the next thing drawn is a page 0 tile. Seen failing with no drop (the old tiles still attached), with the drop made before the draw and the preview not there (a blank step), and with the shown tab's entries only (tab 2 still holds page 0).
#[test]
fn a_changed_look_leaves_no_old_tile_and_no_bare_frame() {
    let mut reader = two_tabs_on_one_file();
    let file = PathBuf::from("/d/a.pdf");
    let old_shown = reader.cache.attached.get(&0).cloned().expect("page 0 is on a layer");
    assert!(old_shown.len() >= 2, "a preview and tiles: {old_shown:?}");
    assert!(!drawn_of(&reader, 2, 0).is_empty(), "the warm tab holds page 0 too");
    let page_1 = (drawn_of(&reader, 1, 1).len(), reader.cache.attached.get(&1).cloned());
    let plan = stand_in_plan(&mut reader, &file, 0).expect("a preview can be drawn at this zoom");
    let job = plan.ready().expect("a surface");
    assert!(matches!(&job.kind, JobKind::Draw { what: DrawKind::Preview, .. }) && job.position == 0);
    reader.work.note_started(&job, None);
    reader.host.take_ops();
    reader.applied();
    let blank_before = reader.counts().blank_seen_steps;
    // The thread's one call: dropped and finished under one hold of the lock.
    drop_look(&mut reader, &file, 0);
    reader.finish(
        job,
        Outcome::Drawn {
            kind: TileKind::Colour,
            low_res: None,
        },
    );
    let ops: Vec<&Op> = reader.host.ops.iter().collect();
    let detached: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter_map(|(at, op)| matches!(op, Op::Detach { id } if old_shown.contains(id)).then_some(at))
        .collect();
    let attached: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter_map(|(at, op)| matches!(op, Op::Attach { page: 0, .. }).then_some(at))
        .collect();
    assert_eq!(
        detached.len(),
        old_shown.iter().filter(|id| reader.cache.entries.get(id).is_none()).count(),
        "every old picture detached: {ops:?}"
    );
    assert_eq!(attached.len(), 1, "the new preview attached, nothing else: {ops:?}");
    assert!(detached.iter().all(|at| *at < attached[0]), "the old ones off before the new one on: {ops:?}");
    assert!(
        !ops.iter().any(|op| matches!(op, Op::HidePage { .. } | Op::Clear)),
        "the page itself stays: {ops:?}"
    );
    let now_shown = reader.cache.attached.get(&0).cloned().unwrap_or_default();
    assert_eq!(now_shown.len(), 1, "page 0 shows one thing: {now_shown:?}");
    assert!(reader.cache.entries[&now_shown[0]].preview, "the new preview");
    assert!(old_shown.iter().all(|id| !reader.cache.entries.contains_key(id)), "no old picture is kept");
    assert_eq!(drawn_of(&reader, 1, 0), now_shown, "tab 1 holds only the new preview of page 0");
    assert!(drawn_of(&reader, 2, 0).is_empty(), "and the warm tab holds none of the old look");
    assert_eq!(
        (drawn_of(&reader, 1, 1).len(), reader.cache.attached.get(&1).cloned()),
        page_1,
        "page 1 is untouched"
    );
    assert_eq!(reader.counts().blank_seen_steps, blank_before, "no bare frame");
    reader.host.take_ops();
    reader.applied();
    let next = a_draw_job(&mut reader, DrawKind::Tile);
    assert_eq!(next.position, 0, "page 0's tiles are drawn next, by the new look");
}

/// Failure: where no preview is possible the page is left holding its old tiles (two looks at once), or the host isn't told they went. They're dropped and told, and the page is asked for again.
#[test]
fn with_no_preview_possible_the_old_tiles_are_just_dropped_and_told() {
    let mut reader = two_tabs_on_one_file();
    let file = PathBuf::from("/d/a.pdf");
    let old_shown = reader.cache.attached.get(&0).cloned().expect("page 0 is on a layer");
    reader.host.take_ops();
    reader.applied();
    bare(&mut reader, &file, 0);
    assert!(reader.cache.attached.get(&0).is_none_or(Vec::is_empty), "nothing of the old look is on page 0");
    assert!(drawn_of(&reader, 2, 0).is_empty() && drawn_of(&reader, 1, 0).is_empty());
    let detached = reader
        .host
        .ops
        .iter()
        .filter(|op| matches!(op, Op::Detach { id } if old_shown.contains(id)))
        .count();
    assert!(detached >= 1, "the host is told: {:?}", reader.host.ops);
}
