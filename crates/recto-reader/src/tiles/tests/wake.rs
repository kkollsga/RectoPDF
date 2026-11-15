use super::*;
use crate::tiles::wake::WakeStock;

/// The place a tab was left at, as the record keeps it: page 0, letter pages.
fn saved_at_the_top() -> SavedPlace {
    SavedPlace {
        page: 0,
        anchor: (0, 0.0),
        x: 0.0,
        y: 0.0,
        document_height: 0.0,
        magnification: 1.0,
        page_size: Some(letter()),
        line: (0, 0.0),
    }
}

/// What a facts-only sleep file gives a wake: letter pages, page 1 (0-based) of another size.
fn facts() -> SleepView {
    SleepView {
        usual: Some(letter()),
        sizes: vec![(1, PageBox { width: 300.0, height: 400.0 })],
    }
}

/// Failure (owner, 2026-11-15: "An idle page loads at the correct scroll position with placeholder papers from the start ... the placeholder paper (with the
/// correct color size and position gets filled with content"): a tab woken from its sleep file's facts comes up with no pages until a worker says their
/// sizes (a blank margin), or its pages at the wrong size, on the wrong paper, or with something drawn on them before anything is. Its first frame, with
/// nothing drawn, has every page in view on its layer, at the size the file says, in the paper's colour, and nothing on any of them; on dark paper too.
/// (Warm paper is white paper with the host's tint over each page layer, which the host lays as it places one.)
#[test]
fn a_wake_from_facts_has_every_page_in_view_on_its_paper_at_its_size_before_anything_is_drawn() {
    for paper in [PaperKind::White, PaperKind::Dark] {
        let mut reader = Reader::new();
        reader.set_paper(paper);
        reader.wake.window_up();
        let tall = Viewport { height: 2_000.0, ..view(0.0) };
        reader.set_viewport(tall);
        // For another tab: not used.
        reader.wake.give_sleep(
            9,
            SleepView {
                usual: Some(PageBox { width: 100.0, height: 100.0 }),
                sizes: Vec::new(),
            },
        );
        reader.note_wake(1, Instant::now());
        reader.wake.give_sleep(1, facts());
        reader.activate(1, 5, 0, map(), &[], Some(saved_at_the_top()), Spread::Single);
        let mut ops = reader.host.take_ops();
        let scrolled = reader.place.jump.map_or(0, std::num::NonZeroU64::get);
        ops.extend(reader.set_viewport(Viewport { scrolled_to: scrolled, ..tall }));
        let doc = reader.docs.shown().unwrap();
        let in_view: Vec<u32> = reader.visible_pages(doc).collect();
        assert!(in_view.len() >= 2, "{paper:?}: the view shows {in_view:?}");
        for page in &in_view {
            let shown = ops.iter().rev().find_map(|op| match op {
                Op::ShowPage {
                    page: at,
                    width,
                    height,
                    paper_rgb,
                    ..
                } if at == page => Some((*width, *height, *paper_rgb)),
                _ => None,
            });
            let size = if *page == 1 { (300.0, 400.0) } else { (612.0, 792.0) };
            assert_eq!(
                shown,
                Some((size.0, size.1, paper.rgb())),
                "{paper:?}: page {page} on its paper at its size in the first frame: {ops:?}"
            );
        }
        assert!(
            !ops.iter().any(|op| matches!(op, Op::Attach { .. })),
            "{paper:?}: nothing's drawn yet, so nothing's on a page: {ops:?}"
        );
        reader.host.unapplied = true;
        reader.applied();
        let wake = reader.wake.last_wake().unwrap();
        assert_eq!(wake.had, WakeStock::Facts, "{paper:?}: {wake:?}");
        assert!(wake.from_facts);
        assert_eq!(wake.first_frame_us, None, "{paper:?}: no page has anything drawn yet: {wake:?}");
    }
}

/// Failure: a tab woken with no sleep file is taken for one woken from its facts, so the record says its first frame was on its paper when it wasn't; or
/// its pages go on paper at a guessed size before a worker has said what they are. With no facts, no page is on paper until its size is known.
#[test]
fn a_wake_with_no_facts_puts_no_page_on_paper_till_its_size_is_known() {
    let mut reader = Reader::new();
    reader.wake.window_up();
    reader.set_viewport(view(0.0));
    reader.note_wake(1, Instant::now());
    reader.activate(1, 5, 0, map(), &[], None, Spread::Single);
    let mut ops = reader.host.take_ops();
    let scrolled = reader.place.jump.map_or(0, std::num::NonZeroU64::get);
    ops.extend(reader.set_viewport(Viewport {
        scrolled_to: scrolled,
        ..view(0.0)
    }));
    assert!(!ops.iter().any(|op| matches!(op, Op::ShowPage { .. })), "{ops:?}");
    reader.host.unapplied = true;
    reader.applied();
    let wake = reader.wake.last_wake().unwrap();
    assert_eq!(wake.had, WakeStock::Nothing, "{wake:?}");
    assert!(!wake.from_facts);
}

/// Failure: a window can't go up until every tile's drawn, which on a heavy page is most of a second, or it goes up over a page with nothing on it, or
/// can't say why it's waiting.
#[test]
fn a_window_doesnt_go_up_over_bare_paper_and_says_why() {
    let mut reader = Reader::new();
    reader.set_viewport(view(0.0));
    reader.wake.give_sleep(1, facts());
    reader.activate(1, 5, 0, map(), &[], Some(saved_at_the_top()), Spread::Single);
    reader.host.take_ops();
    assert!(!reader.is_showable(), "still being sent to its place, with nothing on it");
    // And it says why, for the record of the window's first showing: a slow page and a missed wake read differently there.
    assert_eq!(reader.not_showable(), Some(crate::tiles::NotShowable::Jump));
    let scrolled = reader.place.jump.map_or(0, std::num::NonZeroU64::get);
    reader.set_viewport(Viewport {
        scrolled_to: scrolled,
        ..view(0.0)
    });
    reader.host.take_ops();
    reader.host.unapplied = false;
    assert!(!reader.is_showable(), "a bare page isn't what goes up");
    assert_eq!(reader.not_showable(), Some(crate::tiles::NotShowable::NotCovered), "nothing's drawing it here");
    settle(&mut reader);
    reader.host.take_ops();
    reader.host.unapplied = false;
    assert!(reader.is_showable(), "drawn, it can");
}

/// Failure: the report grew past what's held for it, one for each page view.
#[test]
fn a_wake_report_stays_small() {
    assert!(std::mem::size_of::<WakeReport>() <= 48);
}

/// Failure: a tab's sleep file is written with sizes of pages that are other pages now (deleted, moved or turned while it was off screen), so its wake
/// lays them out at another page's size; or a tab of another file's view gives its sizes. The material is the sizes the view knows, of the version a
/// worker said, and nothing when the pages aren't the session's now.
#[test]
fn a_tabs_material_is_its_sizes_and_only_of_its_pages_as_they_are_now() {
    let mut reader = reader_on(5);
    reader.note_version(Path::new("/d/a.pdf"), 7);
    settle_all_sized(&mut reader, letter());
    let material = reader.sleep_material(1, &map()).unwrap();
    assert_eq!(material.version, Some(7));
    assert_eq!(material.usual, Some(letter()));
    let mut fewer = map();
    fewer.slots = Some(vec![(0, 0), (0, 1), (0, 2)]);
    assert_eq!(
        reader.sleep_material(1, &fewer),
        Some(SleepMaterial::default()),
        "pages changed off screen: nothing of them"
    );
    let mut other = map();
    other.own = Some(recto_platform::FileStamp {
        size: 99,
        mtime_ns: 1,
        device: 1,
        inode: 1,
    });
    assert_eq!(reader.sleep_material(1, &other), None, "another file's");
}

/// Failure: the record of a window's first showing reads a reason back as another (a byte kept for it in the app's state), so a slow page is told as a
/// missed wake or the other way round.
#[test]
fn why_a_window_cant_go_up_reads_back_as_itself() {
    use crate::tiles::NotShowable;
    assert_eq!(NotShowable::from_code(NotShowable::code(None)), None);
    let all = (1..=u8::MAX).map_while(NotShowable::from_code).collect::<Vec<_>>();
    assert_eq!(all.len(), 9, "each reason has its own byte: {all:?}");
    for reason in all {
        assert_eq!(NotShowable::from_code(NotShowable::code(Some(reason))), Some(reason));
    }
}

/// Failure (the "wake that never ended" of 2026-11-15, 1 in about 30 on the PDF route): a wake is noted on the session thread and its tab activated in a
/// second step, each under the page view's lock; the host applying a frame in between (the UI thread's `applied`) found the hot tab wasn't the waking one
/// and threw the wake away, so `last_wake` stayed the tab before's and nothing recorded the tab's first frame or sharp, though it was drawn. A wake noted
/// and not yet activated is kept through an apply, and recorded once its tab is shown.
#[test]
fn a_frame_applied_between_a_wakes_note_and_its_tab_shown_keeps_the_wake() {
    let mut reader = reader_on(5);
    settle(&mut reader);
    reader.note_wake(2, Instant::now());
    // The host applies a frame of tab 1 before tab 2 is activated.
    reader.host.unapplied = true;
    reader.applied();
    reader.activate(
        2,
        5,
        0,
        PageMap {
            files: vec![PathBuf::from("/d/2.pdf")],
            ..map()
        },
        &[1],
        None,
        Spread::Single,
    );
    reader.host.take_ops();
    let scrolled = reader.place.jump.map_or(0, std::num::NonZeroU64::get);
    reader.set_viewport(Viewport {
        scrolled_to: scrolled,
        ..view(0.0)
    });
    settle(&mut reader);
    reader.host.take_ops();
    reader.host.unapplied = true;
    reader.applied();
    let wake = reader.wake.last_wake().expect("a wake");
    assert_eq!(wake.tab, 2, "the wake of the tab shown, not the one before: {wake:?}");
    assert!(wake.first_frame_us.is_some() && wake.sharp_us.is_some(), "{wake:?}");
}
