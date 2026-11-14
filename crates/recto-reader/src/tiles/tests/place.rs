use super::*;

/// Failure: an agent's `goto` to a page reads as the reader having scrolled there and then scrolled back, so the session's place fights the jump. Arriving isn't a move, and a real scroll afterwards is.
#[test]
fn a_jump_isnt_reported_back_as_a_move() {
    let mut reader = reader_on(100);
    reader.place.scrolled_to = 0;
    settle(&mut reader);
    let mut base = view(0.0);
    base.scrolled_to = 0;
    reader.set_viewport(base);
    reader.place.take_moved_page();
    reader.jump_to(50, 0.0);
    let Some(Op::ScrollTo { y, seq }) = reader.host.take_ops().into_iter().find(|op| matches!(op, Op::ScrollTo { .. })) else {
        panic!("no scroll")
    };
    let mut there = view(y);
    there.scrolled_to = seq - 1;
    reader.set_viewport(there);
    assert_eq!(reader.place.take_moved_page(), None, "not applied yet");
    there.scrolled_to = seq;
    reader.set_viewport(there);
    assert_eq!(reader.place.take_moved_page(), None, "arrived");
    there.y = y + 5000.0;
    reader.set_viewport(there);
    assert!(reader.place.take_moved_page().is_some_and(|page| page > 50), "the reader moved");
}

/// Failure (slow mode, 2026-11-11: five update tests): the host puts the view on its own pixels, so it arrives a fraction of a point off where it was sent, and the next word from it that isn't the arrival (the window shown, a layout's tick, a pixel's resize) was taken for the reader moving. The place was taken again from the rounded height: the top of page 9 became the foot of page 8 (it was in the gap above), or a ten-thousandth into the page, and the next launch went there. A view within a point of where the place says is still on the place, said the host's way.
#[test]
fn a_view_the_host_rounded_to_its_pixels_is_still_on_the_place() {
    let mut reader = reader_on(100);
    reader.place.scrolled_to = 0;
    settle(&mut reader);
    let mut base = view(0.0);
    base.scrolled_to = 0;
    reader.set_viewport(base);
    reader.place.take_moved_page();
    reader.jump_to(8, 0.0);
    let Some(Op::ScrollTo { y, seq }) = reader.host.take_ops().into_iter().find(|op| matches!(op, Op::ScrollTo { .. })) else {
        panic!("no scroll")
    };
    let anchor = reader.docs.shown().unwrap().layout.anchor_for(y, 8);
    assert_eq!(anchor, (8, 0.0));
    // On a half-point pixel just above, which is in the gap above the page, and just below.
    for rounded in [(y * 2.0).floor() / 2.0, y + 0.07] {
        let mut there = view(rounded);
        there.scrolled_to = seq;
        reader.set_viewport(there);
        // Then a word from the host that isn't an arrival: the window a pixel taller.
        there.height += 1.0;
        reader.set_viewport(there);
        let (_, place) = reader.places()[0];
        assert_eq!(place.anchor, (8, 0.0), "at {rounded}, for the place at {y}");
        assert_eq!(place.page, 8);
        assert_eq!(reader.place.take_moved_page(), None, "and it isn't a move");
    }
    // A real scroll is, even a short one.
    let mut moved = view(y + 3.0);
    moved.scrolled_to = seq;
    reader.set_viewport(moved);
    assert!(reader.places()[0].1.anchor.1 > 0.0, "{:?}", reader.places()[0].1);
}

/// Failure: synced scroll can't tell the reader's own scroll from a pane arriving where it was sent, so two panes keep sending each other back and forth, or a resize or a zoom drags the other pane along. Only a real scroll counts, and a pane sent somewhere by `follow` arrives there without leading.
#[test]
fn only_the_readers_own_scroll_leads_a_synced_pane() {
    let mut reader = reader_on(100);
    reader.place.scrolled_to = 0;
    settle(&mut reader);
    let mut base = view(0.0);
    base.scrolled_to = 0;
    reader.set_viewport(base);
    reader.place.take_user_scrolled();
    let mut scrolled = base;
    scrolled.y = 2000.0;
    reader.set_viewport(scrolled);
    assert!(reader.place.take_user_scrolled(), "the reader scrolled");
    assert!(!reader.place.take_user_scrolled(), "asking clears it");
    let page = reader.follow(40, 0.25).expect("a tab is showing");
    assert_eq!(page, 40);
    let Some(Op::ScrollTo { y, seq }) = reader.host.take_ops().into_iter().rfind(|op| matches!(op, Op::ScrollTo { .. })) else {
        panic!("no scroll")
    };
    let mut there = scrolled;
    there.y = y;
    there.scrolled_to = seq;
    reader.set_viewport(there);
    assert!(!reader.place.take_user_scrolled(), "arriving isn't leading");
    assert_eq!(reader.docs.shown().map(|doc| doc.reading_anchor()).map(|(page, _)| page), Some(40));
    let mut zoomed = there;
    zoomed.magnification = 1.5;
    zoomed.y += 300.0;
    reader.set_viewport(zoomed);
    assert!(!reader.place.take_user_scrolled(), "a zoom isn't a scroll");
    let mut pinch = zoomed;
    pinch.live = true;
    pinch.y += 50.0;
    reader.set_viewport(pinch);
    assert!(!reader.place.take_user_scrolled(), "nor is a pinch");
    assert_eq!(reader.follow(10_000, 2.0), Some(99), "held to the document");
}

/// Failure (found by restore's whole-state test, with comparison): a view that arrives where it was sent shows another page where the reader's eye is, as a narrower pane does, and the page view's place says one page while the session says another, so the place isn't brought back after a restart. Arriving on another page than was asked says so.
#[test]
fn arriving_on_another_page_than_asked_says_so() {
    let mut reader = reader_on(100);
    reader.place.scrolled_to = 0;
    settle(&mut reader);
    reader.place.take_moved_page();
    assert_eq!(reader.follow(40, 0.9), Some(40));
    assert_eq!(reader.place.take_moved_page(), Some(40), "sent there");
    let Some(Op::ScrollTo { y, seq }) = reader.host.take_ops().into_iter().rfind(|op| matches!(op, Op::ScrollTo { .. })) else {
        panic!("no scroll")
    };
    let mut there = view(y);
    there.scrolled_to = seq;
    reader.set_viewport(there);
    assert_eq!(reader.place.take_moved_page(), Some(41), "a third of the way down the view is the next page");
    let mut again = there;
    again.y += 1.0;
    reader.set_viewport(again);
    assert_eq!(reader.place.take_moved_page(), None, "and nothing more until it moves on");
}

/// Failure (found by restore's whole-state test): side by side, going to a right-hand page arrives at a view whose top row starts with the left one, so the page view's place says the left page while the session says the right, and the place isn't brought back after a restart. And a change of layout moves the place to the other page of a row.
#[test]
fn side_by_side_keeps_the_page_of_its_row_the_reader_was_on() {
    let mut reader = reader_on(100);
    reader.place.scrolled_to = 0;
    reader.set_spread(Spread::TwoUp);
    settle(&mut reader);
    reader.place.take_moved_page();
    reader.jump_to(51, 0.0);
    let Some(Op::ScrollTo { y, seq }) = reader.host.take_ops().into_iter().rfind(|op| matches!(op, Op::ScrollTo { .. })) else {
        panic!("no scroll")
    };
    let mut there = view(y);
    there.scrolled_to = seq;
    reader.set_viewport(there);
    assert_eq!(reader.place.take_moved_page(), None, "arrived");
    assert_eq!(reader.places()[0].1.page, 51, "the right-hand page it went to");
    there.y = y + 20.0;
    reader.set_viewport(there);
    assert_eq!(reader.place.take_moved_page(), None, "reading on in the row isn't going to the left page");
    reader.set_spread(Spread::Single);
    assert_eq!(reader.places()[0].1.page, 51);
}

/// Failure (PLAN, from the side-by-side review, finding 4): side by side, on the right-hand page of a pair, a round trip through a view too small to reach the place, as a window made narrow and tall, where the last page is taken for the reader's, comes back on the left page of the pair, and the session's page moves with it. Each page, as the host applies what it's sent, comes back to itself.
#[test]
fn a_round_trip_through_a_small_view_keeps_the_right_hand_page() {
    fn host(reader: &mut Reader, mut at: Viewport) -> Viewport {
        let mut ops = reader.set_viewport(at);
        for _ in 0..6 {
            let Some(Op::ScrollTo { y, seq }) = ops.into_iter().rfind(|op| matches!(op, Op::ScrollTo { .. })) else {
                break;
            };
            at.y = y;
            at.scrolled_to = seq;
            ops = reader.set_viewport(at);
        }
        at
    }
    for (wide, high) in [(200.0, 900.0), (100.0, 60.0), (300.0, 200.0)] {
        for target in [51u32, 11, 97] {
            let mut reader = reader_on(100);
            reader.place.scrolled_to = 0;
            reader.set_spread(Spread::TwoUp);
            settle(&mut reader);
            reader.jump_to(target, 0.0);
            let mut at = follow_host(&mut reader, view(0.0));
            reader.place.take_moved_page();
            for scroll in [0.0, 37.0] {
                at.y += scroll;
                at = host(&mut reader, at);
                let small = host(
                    &mut reader,
                    Viewport {
                        width: wide,
                        frame_width: wide,
                        height: high,
                        ..at
                    },
                );
                at = host(
                    &mut reader,
                    Viewport {
                        width: 1168.0,
                        frame_width: 1168.0,
                        height: 900.0,
                        ..small
                    },
                );
                assert_eq!(reader.places()[0].1.page, target, "{wide} x {high}, page {target}, {scroll} down");
                assert_eq!(reader.docs.shown().map(|doc| doc.reading_anchor()).map(|(page, _)| page), Some(target));
                assert!(
                    reader.place.take_moved_page().is_none_or(|page| page == target),
                    "the session's page left {target}"
                );
            }
        }
    }
}

/// Failure (PLAN, side-by-side follow-ups): the host is told the document is as wide as a fold-out wherever the view is, leaving room to scroll across beside every other page; or it isn't told it's wider when the fold-out comes into view, so its right side can't be reached.
#[test]
fn the_host_hears_the_documents_width_for_what_it_shows() {
    let mut reader = reader_on(40);
    settle(&mut reader);
    let doc = reader.docs.views.get_mut(&1).unwrap();
    doc.layout.set_page_size(21, PageBox { width: 3000.0, height: 792.0 });
    let (wide, frame) = (doc.layout.rect(21), doc.layout.document_width_for(0.0, 1.0));
    reader.lay_out();
    reader.host.take_ops();
    let widths = |ops: Vec<Op>| {
        ops.into_iter()
            .filter_map(|op| if let Op::Document { width, .. } = op { Some(width) } else { None })
            .collect::<Vec<_>>()
    };
    let there = widths(reader.set_viewport(view(wide.y - 200.0)));
    assert!(there.last().is_some_and(|width| *width >= wide.x + wide.width), "{there:?} for {wide:?}");
    assert!(
        widths(reader.set_viewport(view(wide.y - 150.0))).is_empty(),
        "nothing new to say while it's still in view"
    );
    assert_eq!(widths(reader.set_viewport(view(0.0))), [frame], "back to the window's width away from it");
}

/// Failure (review of f05c998, finding 1): Rotate, Undo or Redo of a turn sends the reader to the top of their page and back to the left edge, as the tab showing was taken up again as if it had just been come to.
#[test]
fn a_turn_keeps_the_place_in_the_page_and_across() {
    let mut reader = reader_on(10);
    reader.place.scrolled_to = 0;
    settle(&mut reader);
    let page = reader.docs.shown().unwrap().layout.rect(3);
    let mut at = view(page.y + page.height * 0.4);
    at.x = 30.0;
    at.magnification = 2.0;
    reader.set_viewport(at);
    reader.place.take_moved_page();
    let before = reader.places()[0].1;
    let turned = PageMap {
        turns: Some(Arc::from(vec![1u8; 10])),
        ..map()
    };
    reader.activate(1, 10, 3, turned, &[], None, Spread::Single);
    follow_host(&mut reader, at);
    let after = reader.places()[0].1;
    assert_eq!(after.page, 3);
    assert!(
        (after.anchor.1 - before.anchor.1).abs() < 0.02,
        "as far down the page: {:?} then {:?}",
        before.anchor,
        after.anchor
    );
    assert_eq!(reader.docs.views[&1].place.scroll_x, 30.0, "and as far across");
}

/// Failure (review of f05c998, B2): side by side, after any scroll, a change of layout moves the reader off the page they were on: the place was kept on the first page of its row, so the right-hand page's reader lands on the left one, or two pages back going to the cover layout. Fed the host's next view, as the app is.
#[test]
fn a_change_of_layout_after_a_scroll_keeps_the_page() {
    for (from, to) in [
        (Spread::TwoUp, Spread::Single),
        (Spread::TwoUp, Spread::TwoUpCover),
        (Spread::TwoUpCover, Spread::TwoUp),
    ] {
        let mut reader = reader_on(100);
        reader.place.scrolled_to = 0;
        reader.set_spread(from);
        settle(&mut reader);
        reader.jump_to(51, 0.0);
        let at = follow_host(&mut reader, view(0.0));
        let mut scrolled = at;
        scrolled.y += 20.0;
        reader.set_viewport(scrolled);
        reader.place.take_moved_page();
        let was = reader.docs.shown().unwrap().layout.rect(51);
        let down = (scrolled.y - was.y) / was.height;
        reader.set_spread(to);
        follow_host(&mut reader, scrolled);
        let place = reader.places()[0].1;
        assert_eq!(place.page, 51, "{from:?} to {to:?}");
        let rect = reader.docs.shown().unwrap().layout.rect(51);
        assert!(
            (reader.viewport.y - (rect.y + down * rect.height)).abs() < 1.0,
            "{from:?} to {to:?}: as far down the page: {} against {}",
            reader.viewport.y,
            rect.y + down * rect.height
        );
    }
}

/// Failure: going to another tab and back sends the reader to the top of their page, so on a tall page, or any page read past its top, the place is lost. On the page it was left on, it comes back to the same place, and anywhere else it starts at the top.
#[test]
fn coming_back_to_a_tab_keeps_the_place_in_the_page() {
    let mut reader = reader_on(100);
    settle(&mut reader);
    let tops = |reader: &Reader, page| reader.docs.shown().unwrap().layout.top(page);
    let inside = tops(&reader, 40) + 300.0;
    reader.set_viewport(view(inside));
    let page = reader.place.take_moved_page().expect("the reader moved");
    settle(&mut reader);
    let other = PageMap {
        files: vec![PathBuf::from("/d/b.pdf")],
        slots: None,
        own: None,
        turns: None,
        expected: Vec::new(),
        follows: None,
    };
    reader.activate(2, 10, 0, other.clone(), &[1], None, Spread::Single);
    reader.host.take_ops();
    reader.activate(1, 100, page, map(), &[2], None, Spread::Single);
    let scrolled: Vec<f64> = reader
        .host
        .take_ops()
        .into_iter()
        .filter_map(|op| if let Op::ScrollTo { y, .. } = op { Some(y) } else { None })
        .collect();
    assert!((scrolled.last().unwrap() - inside).abs() < 1.0, "{scrolled:?} against {inside}");

    reader.activate(2, 10, 0, other, &[1], None, Spread::Single);
    reader.host.take_ops();
    reader.activate(1, 100, page + 3, map(), &[2], None, Spread::Single);
    let scrolled: Vec<f64> = reader
        .host
        .take_ops()
        .into_iter()
        .filter_map(|op| if let Op::ScrollTo { y, .. } = op { Some(y) } else { None })
        .collect();
    let top = tops(&reader, page + 3);
    assert!(
        (scrolled.last().unwrap() - top).abs() < 1.0,
        "sent somewhere else, it starts at the top: {scrolled:?} against {top}"
    );
}

/// Failure: a new window's first viewport, at the top and no zoom, comes before it's opened and is taken for the reader moving, so closing the window and opening it again loses the place and the zoom. Before and after it's told, what it says doesn't count till it's arrived.
#[test]
fn reopening_the_window_keeps_the_place_and_the_zoom() {
    let mut reader = reader_on(100);
    settle(&mut reader);
    reader.set_viewport(Viewport {
        magnification: 2.0,
        ..view(0.0)
    });
    let inside = reader.docs.shown().unwrap().layout.top(40) + 300.0;
    reader.set_viewport(Viewport {
        magnification: 2.0,
        ..view(inside)
    });
    reader.place.take_moved_page();
    settle(&mut reader);
    // As the old window got to, with its own count of scrolls.
    reader.place.scrolled_to = 0;
    reader.window_closed();
    let new_window = Viewport { scrolled_to: 0, ..view(0.0) };
    reader.set_viewport(new_window);
    assert_eq!(reader.place.take_moved_page(), None, "the new window isn't the reader moving");
    reader.window_opened();
    let ops = reader.set_viewport(new_window);
    assert_eq!(reader.place.take_moved_page(), None, "nor is it before it has got there");
    assert!(ops.iter().any(|op| matches!(op, Op::Magnify { value } if *value == 2.0)), "{ops:?}");
    let Some(Op::ScrollTo { y, seq }) = ops.iter().rev().find(|op| matches!(op, Op::ScrollTo { .. })) else {
        panic!("{ops:?}")
    };
    assert!((y - inside).abs() < 1.0, "{y} against {inside}");
    reader.set_viewport(Viewport {
        magnification: 2.0,
        scrolled_to: *seq,
        ..view(*y)
    });
    assert_eq!(reader.place.take_moved_page(), None, "arriving isn't a move");
    assert_eq!(reader.docs.shown().unwrap().place.magnification, 2.0);
}

/// Failure: a tab comes back where the document's top is, or at no zoom, or near its place by rounding rather than on it. Or the place is taken from where the view still is on its way there, so the next layout goes somewhere else. What's said comes straight back out, to the pixel, and a place for another page, as a `goto` while cold leaves, isn't used.
#[test]
fn a_saved_place_comes_back_to_the_pixel() {
    let mut reader = Reader::new();
    reader.window_opened();
    reader.set_viewport(view(0.0));
    let frame_view = view(0.0);
    let mut layout = PageLayout::new(100);
    layout.seed_usual(letter());
    layout.set_frame(frame_view.frame_width);
    let height = layout.document_height();
    let y = layout.top(40) + 123.456_789;
    let saved = SavedPlace {
        page: 40,
        anchor: layout.anchor_at(y),
        x: 33.5,
        y,
        document_height: height,
        magnification: 1.75,
        page_size: Some(letter()),
        line: (0, 0.0),
    };
    reader.activate(1, 100, 40, map(), &[], Some(saved), Spread::Single);
    let ops = reader.host.take_ops();
    assert!(ops.iter().any(|op| matches!(op, Op::Magnify { value } if *value == 1.75)), "{ops:?}");
    assert!(ops.iter().any(|op| matches!(op, Op::ScrollAcross { x } if *x == 33.5)), "{ops:?}");
    let Some(Op::ScrollTo { y: went, seq }) = ops.iter().rev().find(|op| matches!(op, Op::ScrollTo { .. })) else {
        panic!("{ops:?}")
    };
    assert_eq!(*went, y, "exactly");
    // Not there yet: where the view says it is, isn't the place.
    reader.place.scrolled_to = 0;
    reader.set_viewport(Viewport {
        magnification: 1.75,
        scrolled_to: 0,
        ..view(0.0)
    });
    assert_eq!(reader.places()[0].1.y, y);
    assert_eq!(reader.places()[0].1.anchor, saved.anchor);
    // Arrived.
    reader.set_viewport(Viewport {
        magnification: 1.75,
        x: 33.5,
        scrolled_to: *seq,
        ..view(y)
    });
    let (_, place) = reader.places()[0];
    assert_eq!((place.page, place.y, place.x, place.magnification), (40, y, 33.5, 1.75));
    assert_eq!(place.anchor, saved.anchor);
    assert_eq!(reader.place.take_moved_page(), None, "arriving isn't a move");
    // Left and back in the same session.
    reader.activate(2, 10, 0, map(), &[1], None, Spread::Single);
    reader.activate(1, 100, 40, map(), &[2], None, Spread::Single);
    let ops = reader.host.take_ops();
    let Some(Op::ScrollTo { y: went, .. }) = ops.iter().rev().find(|op| matches!(op, Op::ScrollTo { .. })) else {
        panic!("{ops:?}")
    };
    assert_eq!(*went, y);
    // Another page since: its top, and no zoom it shouldn't have kept is the place's business.
    let mut fresh = Reader::new();
    fresh.window_opened();
    fresh.set_viewport(view(0.0));
    fresh.activate(1, 100, 7, map(), &[], Some(saved), Spread::Single);
    let ops = fresh.host.take_ops();
    let Some(Op::ScrollTo { y: went, .. }) = ops.iter().rev().find(|op| matches!(op, Op::ScrollTo { .. })) else {
        panic!("{ops:?}")
    };
    assert_eq!(*went, fresh.docs.shown().unwrap().layout.top(7));
    // What a record can say that isn't a place doesn't get as far as a host.
    let mut hostile = Reader::new();
    hostile.window_opened();
    hostile.set_viewport(view(0.0));
    hostile.activate(
        1,
        100,
        40,
        map(),
        &[],
        Some(SavedPlace {
            anchor: (9_000_000, f64::NAN),
            x: f64::NAN,
            y: f64::NAN,
            document_height: f64::INFINITY,
            magnification: f64::NAN,
            ..saved
        }),
        Spread::Single,
    );
    for op in hostile.host.take_ops() {
        match op {
            Op::ScrollTo { y, .. } => assert!(y.is_finite(), "{y}"),
            Op::ScrollAcross { x } => assert!(x.is_finite(), "{x}"),
            Op::Magnify { value } => assert!(value.is_finite() && (MIN_ZOOM..=MAX_ZOOM).contains(&value), "{value}"),
            _ => {}
        }
    }
}

/// Failure: the host's size change moves the reader away from what they were reading.
#[test]
fn a_resize_keeps_the_reader_on_their_page() {
    let mut reader = reader_on(100);
    settle(&mut reader);
    let y = reader.docs.shown().unwrap().layout.top(40);
    reader.set_viewport(view(y));
    settle(&mut reader);
    let ops = reader.set_viewport(Viewport {
        frame_width: 900.0,
        width: 900.0,
        ..view(y)
    });
    let Some(Op::ScrollTo { y: new_y, .. }) = ops.iter().find(|op| matches!(op, Op::ScrollTo { .. })) else {
        panic!("{ops:?}")
    };
    let top = reader.docs.shown().unwrap().layout.top(40);
    assert!((*new_y - top).abs() < 1.0, "{new_y} against {top}");
}

/// Failure: a way of moving where a page view says a tab is doesn't move the place clock, so a read of its places after that move carries the stamp of a
/// read before it, and the session can't tell which is newer: an older read taken in after a newer one undoes the newer (the person's scroll lost to a
/// command's read still landing), and a jump sent and since moved on from is kept as where the reader is. Walks every entry point of the reader's that
/// moves the place, and fails on any whose read changed while the clock stood still; each must move something, so the walk can't pass by doing nothing.
/// **A new way to move the place goes in this list.** Only the clock's order is asserted, never its value (it's the process's: see `place_stamp`).
#[test]
fn every_way_the_place_moves_moves_the_place_clock() {
    fn range(page: u32) -> recto_index::select::Range {
        recto_index::select::Range {
            start: recto_index::select::Place { page, at: 0 },
            end: recto_index::select::Place { page, at: 4 },
        }
    }
    type Step = fn(&mut Reader);
    let steps: [(&str, Step); 12] = [
        ("the reader's own scroll", |reader| drop(reader.set_viewport(view(reader.viewport.y + 500.0)))),
        ("a viewport of another width, which lays out again", |reader| {
            let mut narrower = view(reader.viewport.y);
            narrower.frame_width = 600.0;
            narrower.width = 600.0;
            drop(reader.set_viewport(narrower));
        }),
        ("synced scroll's follow", |reader| {
            reader.follow(7, 0.5);
        }),
        ("a jump to a page's top", |reader| {
            reader.jump_to(3, 0.0);
        }),
        ("a jump a way down a page", |reader| {
            reader.jump_to(4, 0.4);
        }),
        ("a zoom", |reader| {
            reader.zoom(Zoom::In);
        }),
        ("another spread", |reader| reader.set_spread(Spread::TwoUp)),
        ("other margins", |reader| {
            // In a view narrow enough that the pages' room is what fits them, as a comparison's pane is.
            let mut narrow = view(reader.viewport.y);
            narrow.frame_width = 600.0;
            narrow.width = 600.0;
            drop(reader.set_viewport(narrow));
            let before = (place_stamp(), reader.places());
            reader.set_margins(0.0, 0.0);
            let after = (place_stamp(), reader.places());
            assert!(after.1 != before.1, "other margins moved the place");
            assert!(
                after.0 > before.0,
                "other margins moved the place and not the clock: {:?} to {:?}",
                before.1,
                after.1
            );
        }),
        ("another tab shown", |reader| reader.activate(2, 20, 5, map(), &[1], None, Spread::Single)),
        ("the tab shown at another page", |reader| {
            reader.activate(1, 10, 8, map(), &[], None, Spread::Single)
        }),
        ("Find's hit on a page not showing", |reader| reader.reveal_range(range(6))),
        ("Find's hit placed once its page's text is here", |reader| {
            reader.reveal_range(range(6));
            let to = last_scroll_to(reader);
            drop(reader.set_viewport(view(to)));
            let before = (place_stamp(), reader.places());
            reader.take_text(1, 6, Some((page_with_line_at(40.0), true)));
            // What the text's arrival did is what's walked here: the reveal's own jump to the page is the step above.
            let after = (place_stamp(), reader.places());
            assert!(after.1 != before.1, "the hit's placing moved the place");
            assert!(
                after.0 > before.0,
                "the hit's placing moved the place and not the clock: {:?} to {:?}",
                before.1,
                after.1
            );
        }),
    ];
    let mut quiet = Vec::new();
    for (name, step) in steps {
        let mut reader = reader_on(10);
        settle(&mut reader);
        drop(reader.set_viewport(view(1000.0)));
        settle(&mut reader);
        let (stamp, places, anchor) = (place_stamp(), reader.places(), reader.docs.shown().map(|doc| doc.reading_anchor()));
        step(&mut reader);
        let (now, places_now, anchor_now) = (place_stamp(), reader.places(), reader.docs.shown().map(|doc| doc.reading_anchor()));
        assert!(places_now != places || anchor_now != anchor, "{name} moved nothing, so it walks nothing");
        if now <= stamp {
            quiet.push(format!("{name}: {places:?} to {places_now:?}"));
        }
    }
    assert!(quiet.is_empty(), "moved the place with the clock standing: {quiet:#?}");
}

/// Where the last scroll sent to the host goes.
fn last_scroll_to(reader: &mut Reader) -> f64 {
    reader
        .host
        .take_ops()
        .into_iter()
        .rev()
        .find_map(|op| if let Op::ScrollTo { y, .. } = op { Some(y) } else { None })
        .expect("a scroll")
}

/// A view whose top is `thousandths` down page `page`.
fn top_on(reader: &Reader, page: u32, thousandths: f64) -> Viewport {
    let rect = reader.docs.shown().unwrap().layout.rect(page);
    let mut at = view(0.0);
    at.y = rect.y + thousandths / 1000.0 * rect.height;
    at
}

/// A reader arrived on page 5 with the top of its view `at` thousandths down, the contents' band set to `[lo, hi)` and nothing moved.
fn with_band(lo: u16, hi: u16, at: f64) -> Reader {
    let mut reader = reader_on(100);
    reader.place.scrolled_to = 0;
    settle(&mut reader);
    reader.jump_to(5, 0.0);
    let Some(Op::ScrollTo { seq, .. }) = reader.host.take_ops().into_iter().rfind(|op| matches!(op, Op::ScrollTo { .. })) else {
        panic!("no scroll")
    };
    let mut there = top_on(&reader, 5, at);
    there.scrolled_to = seq;
    reader.set_viewport(there);
    reader.place.take_moved_page();
    reader.docs.views.get_mut(&1).unwrap().band = (5, lo, hi);
    reader
}

/// Failure (the contents' current entry follows a scroll): reading down a page wakes the core only when the page changes, so the mark stays on the first entry
/// of a page to its foot. Leaving the band wakes it; staying in it doesn't (a wake per scroll step would cost every step); and the one wake is for the whole
/// crossing: scroll steps back and forth over the edge, as a trackpad resting on it or rubber-banding, wake once until the core sends a band.
#[test]
fn leaving_the_band_wakes_the_core_once_until_it_sends_another() {
    let mut reader = with_band(300, 600, 310.0);
    for top in (310..=590).step_by(10) {
        reader.set_viewport(top_on(&reader, 5, f64::from(top)));
        assert_eq!(reader.place.take_moved_page(), None, "inside the band at {top}");
    }
    let mut wakes = 0;
    for step in 0..60 {
        // Across the edge at 600 and back, 30 steps each way.
        let top = if step < 30 { 595 + step } else { 625 - (step - 30) };
        reader.set_viewport(top_on(&reader, 5, f64::from(top)));
        wakes += usize::from(reader.place.take_moved_page().is_some());
    }
    assert_eq!(wakes, 1, "one wake for the crossing, whatever the steps over the edge");
    // The core sends a band for where the reader is now: it wakes again on leaving that one.
    reader.docs.views.get_mut(&1).unwrap().band = (5, 595, 640);
    reader.set_viewport(top_on(&reader, 5, 700.0));
    assert!(reader.place.take_moved_page().is_some(), "and leaving the new band wakes again");
}

/// Failure: a band narrower than the pixel's rounding, or none at all (a page of a file with no places, or a page with an unsaved turn), wakes the core on
/// every scroll step, or a view with no band wakes at all.
#[test]
fn a_narrow_band_wakes_once_and_no_band_never() {
    let mut reader = with_band(500, 503, 501.0);
    let wakes: usize = (0..40)
        .map(|step| {
            reader.set_viewport(top_on(&reader, 5, if step % 2 == 0 { 520.0 } else { 495.0 }));
            usize::from(reader.place.take_moved_page().is_some())
        })
        .sum();
    assert_eq!(wakes, 1, "a band of 3 thousandths");
    let mut none = reader_on(100);
    none.place.scrolled_to = 0;
    settle(&mut none);
    none.place.take_moved_page();
    for top in [100.0, 400.0, 5.0] {
        none.set_viewport(top_on(&none, 0, top));
        assert_eq!(none.place.take_moved_page(), None, "no band, no wake");
    }
}

/// Failure: side by side, the top of the view is taken against the other page of the row (a page of another height), so the band is left too soon or too late. The
/// row's page is the one the session has, and the line is the top of the view, down its rect.
#[test]
fn side_by_side_the_top_of_the_view_is_down_the_page_the_reader_is_on() {
    let mut reader = reader_on(100);
    reader.place.scrolled_to = 0;
    reader.set_spread(Spread::TwoUp);
    // The left page of row 0 is twice the right one's height.
    settle_sized(&mut reader, PageBox { width: 612.0, height: 1584.0 });
    reader.place.take_moved_page();
    let left = reader.docs.shown().unwrap().layout.rect(0);
    let right = reader.docs.shown().unwrap().layout.rect(1);
    assert!(left.height > right.height * 1.5, "the pages differ: {} and {}", left.height, right.height);
    reader.docs.views.get_mut(&1).unwrap().band = (0, 0, 400);
    // A top 30% down the left page is more than 40% down the right one: leaving the band by the right page's reckoning, and not by the left's.
    let mut at = view(0.0);
    at.y = left.y + 0.30 * left.height;
    reader.set_viewport(at);
    assert_eq!(reader.place.take_moved_page(), None, "on the left page, 300 of 1000");
    at.y = left.y + 0.45 * left.height;
    reader.set_viewport(at);
    assert!(reader.place.take_moved_page().is_some(), "450 of 1000 is out of the band");
}

/// Failure (S2 of the second review): the core reads the places, then sets the band some time later in the same pass; a scroll step in between is checked
/// against the old band, whose wake was already spent, and the new band is set without a look at where the view is now, so the mark stays where the
/// view was until some later step leaves the new band. A band set with the view already outside it wakes the core once, and one with the view inside doesn't.
#[test]
fn a_band_set_with_the_view_already_outside_it_wakes_the_core_once() {
    let mut reader = with_band(300, 600, 310.0);
    crate::tiles::set_band(&mut reader, Some((5, 300, 600)));
    assert_eq!(reader.place.take_moved_page(), None, "inside: no wake");
    crate::tiles::set_band(&mut reader, Some((5, 400, 600)));
    assert!(reader.place.take_moved_page().is_some(), "the view, at 310, is already above the band");
    reader.set_viewport(top_on(&reader, 5, 320.0));
    assert_eq!(reader.place.take_moved_page(), None, "and that was the wake for it");
    crate::tiles::set_band(&mut reader, Some((6, 0, 600)));
    assert!(reader.place.take_moved_page().is_some(), "a band on another page");
}

/// Where the view can scroll to at most, as the host's clip view holds it: the document's foot less the view, and the room below the last row.
fn scroll_end(reader: &Reader, height: f64) -> f64 {
    let layout = &reader.docs.shown().unwrap().layout;
    layout.document_height() + layout.end_room().1 - height
}

/// Failure (S1 of the second review): the line the current entry is found by was the top of the view, which can't come within a view-height of the
/// document's end, so the entries in the last view-height could never become current by scrolling, and at the very bottom the page number said the last
/// page while the mark sat pages up. Scrolling down a ten-page document whose last three pages hold seven entries, a step at a time, each entry becomes
/// current in turn, none skipped and none out of order, and the last is current at the bottom.
#[test]
fn scrolling_down_to_the_end_makes_every_entry_current_in_order_and_the_last_at_the_bottom() {
    let tops: [(u32, u16); 9] = [(0, 0), (4, 300), (7, 100), (7, 800), (8, 0), (8, 500), (9, 200), (9, 600), (9, 950)];
    let lines: Vec<recto_core::TocLine<'_>> = tops
        .iter()
        .map(|(page, _)| recto_core::TocLine {
            title: "t",
            page: *page,
            parent: recto_core::NONE,
            depth: 0,
        })
        .collect();
    let toc = recto_core::Toc::from_lines(&lines, false).with_tops(&tops.map(|(_, top)| top));
    let mut reader = reader_on(10);
    reader.place.scrolled_to = 0;
    settle(&mut reader);
    let height = view(0.0).height;
    let end = scroll_end(&reader, height);
    assert!(end > 9.0 * height, "a document many views long: {end}");
    let mut seen: Vec<usize> = Vec::new();
    let mut y: f64 = 0.0;
    loop {
        reader.set_viewport(view(y.min(end)));
        let (_, place) = reader.places()[0];
        let current = toc.current_at(place.line.0, recto_core::thousandths(place.line.1), |_| true);
        if let Some(current) = current
            && seen.last() != Some(&current)
        {
            seen.push(current);
        }
        if y >= end {
            break;
        }
        y += 4.0;
    }
    assert_eq!(seen, (0..tops.len()).collect::<Vec<_>>(), "each entry in turn, the last at the bottom");
}

/// Failure: the line the current entry is found by jumps back up as the view goes down (the mark would flicker back), is above the top of the view (an entry
/// the reader has scrolled past isn't current yet), or doesn't reach the last page's foot exactly at the end of the scroll (the last entry is never current).
/// Over random documents (page counts, sizes, a page to a row or two: the order in a row of one) and view heights, from the top to the end of the scroll.
#[test]
fn the_line_goes_only_down_is_never_above_the_view_and_reaches_the_last_foot_at_the_end() {
    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = move |below: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % below
    };
    for case in 0..300 {
        let count = 1 + next(40) as u32;
        let spread = [Spread::Single, Spread::TwoUp, Spread::TwoUpCover][next(3) as usize];
        let mut layout = PageLayout::new(count);
        layout.set_frame(1168.0);
        layout.seed_usual(letter());
        layout.set_spread(spread);
        for _ in 0..next(5) {
            let size = PageBox {
                width: 200.0 + next(800) as f32,
                height: 100.0 + next(2000) as f32,
            };
            layout.set_page_size(next(u64::from(count)) as u32, size);
        }
        let height = 50.0 + next(3000) as f64;
        let end = layout.document_height() + layout.end_room().1 - height;
        let last = (count - 1, 1.0);
        if height > layout.document_height() {
            assert_eq!(
                super::super::place::reading_line(&layout, 0.0, height, 0),
                layout.anchor_for(0.0, 0),
                "case {case}: held whole, the top"
            );
            continue;
        }
        let mut was = (0, 0.0);
        let mut y = -layout.end_room().0;
        loop {
            let at = y.min(end);
            // The reader's page as the page view has it in a row of one; side by side its row's first.
            let page = layout.page_at(at + height / 3.0);
            let line = super::super::place::reading_line(&layout, at, height, page);
            // Side by side, which page of a row a place is said on goes by the reader's page (two-page spreads are a follow-up of their own), so the order
            // is held in a row of one, and the end everywhere.
            if spread == Spread::Single {
                assert!(line >= was, "case {case}: back up from {was:?} to {line:?} at {at} of {end}");
                assert!(line >= layout.anchor_for(at, page), "case {case}: above the top of the view at {at}");
            }
            was = line;
            if at >= end {
                assert_eq!(line, last, "case {case}: the end of the scroll is the last page's foot");
                break;
            }
            y += 1.0 + next(40) as f64;
        }
    }
}
