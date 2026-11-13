use super::*;
use crate::layout::PageBox;
use std::path::Path;

mod zoom;

fn map() -> PageMap {
    PageMap {
        files: vec![PathBuf::from("/d/a.pdf")],
        slots: None,
        own: None,
        turns: None,
        expected: Vec::new(),
        follows: None,
    }
}

impl Reader {
    /// The next job with its surface made, as the engine has it.
    fn next_job(&mut self) -> Option<Job> {
        self.next_plan()?.ready()
    }
}

fn view(y: f64) -> Viewport {
    Viewport {
        x: 0.0,
        y,
        width: 1168.0,
        height: 900.0,
        frame_width: 1168.0,
        magnification: 1.0,
        backing: 2.0,
        live: false,
        scrolled_to: u64::MAX,
    }
}

fn letter() -> PageBox {
    PageBox { width: 612.0, height: 792.0 }
}

/// Runs the job queue with a pretend renderer until there's nothing left to do, and applies what the host would.
fn settle(reader: &mut Reader) -> Vec<Job> {
    settle_all_sized(reader, letter())
}

/// [`settle`], with every page this size.
fn settle_all_sized(reader: &mut Reader, size: PageBox) -> Vec<Job> {
    let mut drawn = Vec::new();
    for _ in 0..10_000 {
        reader.take_ops();
        reader.applied();
        let Some(job) = reader.next_job() else { break };
        let outcome = match &job.kind {
            JobKind::Size => Outcome::Size(size),
            JobKind::Text { .. } => panic!("no text was asked for"),
            JobKind::Links => Outcome::Links(None),
            JobKind::Traits => Outcome::Heavy(false),
            JobKind::Draw { .. } => Outcome::Drawn {
                kind: TileKind::Colour,
                low_res: None,
            },
        };
        drawn.push(Job {
            tab: job.tab,
            position: job.position,
            file: job.file.clone(),
            source_page: job.source_page,
            generation: job.generation,
            kind: match &job.kind {
                JobKind::Size => JobKind::Size,
                JobKind::Traits => JobKind::Traits,
                JobKind::Text { check } => JobKind::Text { check: *check },
                JobKind::Links => JobKind::Links,
                JobKind::Draw {
                    scale,
                    rect,
                    surface,
                    what,
                    paper,
                } => JobKind::Draw {
                    scale: *scale,
                    rect: *rect,
                    surface: surface.clone(),
                    what: *what,
                    paper: *paper,
                },
            },
        });
        reader.finish(job, outcome);
        let more = reader.drain(MAX_ATTACH_PER_TURN);
        drop(more);
        reader.applied();
    }
    for _ in 0..100 {
        if !reader.has_ops() {
            break;
        }
        reader.drain(MAX_ATTACH_PER_TURN);
        reader.applied();
    }
    drawn
}

fn reader_on(pages: u32) -> Reader {
    let mut reader = Reader::new();
    reader.set_viewport(view(0.0));
    reader.activate(1, pages, 0, map(), &[], None, Spread::Single);
    reader.set_viewport(view(0.0));
    reader
}

/// A page with one line of text near its foot, in PDF points from the bottom.
fn page_with_line_at(pdf_y: f32) -> Arc<recto_index::select::PageText> {
    let frame = recto_proto::PageFrame {
        turns: 0,
        left: 0.0,
        bottom: 0.0,
        right: 612.0,
        top: 792.0,
    };
    let chars: Vec<recto_proto::CharRecord> = "find me here".chars().map(|ch| recto_proto::CharRecord::new(ch as u32, 0)).collect();
    let boxes: Vec<recto_proto::CharBox> = (0..chars.len())
        .map(|at| [72.0 + at as f32 * 6.0, pdf_y, 77.0 + at as f32 * 6.0, pdf_y + 10.0])
        .collect();
    Arc::new(recto_index::select::PageText::build(&chars, &boxes, frame, true, false))
}

/// Failure: going to Find's hit leaves it off screen or at the very edge of the view, as it did at the foot of a page jumped to; or stepping between hits already on screen moves the page each time. Selects the hit too.
#[test]
fn finds_hit_is_selected_and_brought_a_third_down_the_view() {
    let mut reader = reader_on(10);
    settle(&mut reader);
    let range = recto_index::select::Range {
        start: recto_index::select::Place { page: 6, at: 0 },
        end: recto_index::select::Place { page: 6, at: 4 },
    };
    reader.reveal_range(range);
    assert_eq!(reader.selection_range(), Some(range), "selected");
    let to_page = reader
        .take_ops()
        .into_iter()
        .rev()
        .find_map(|op| if let Op::ScrollTo { y, .. } = op { Some(y) } else { None })
        .expect("its page, at once");
    reader.set_viewport(view(to_page));
    reader.take_text(1, 6, Some((page_with_line_at(40.0), true)));
    let placed = reader
        .take_ops()
        .into_iter()
        .rev()
        .find_map(|op| if let Op::ScrollTo { y, .. } = op { Some(y) } else { None })
        .expect("then the hit, once its place is known");
    let doc = reader.doc().unwrap();
    let top = doc.layout.rect(6).y + (792.0 - 50.0) * doc.layout.fit();
    let height = view(0.0).height;
    assert!(
        top >= placed + height * 0.25 && top <= placed + height * 0.4,
        "a third down: hit at {top}, view from {placed}"
    );
    // Another hit on what's showing, away from the edges, moves nothing.
    reader.set_viewport(view(placed));
    reader.take_ops();
    let near = recto_index::select::Range {
        start: recto_index::select::Place { page: 6, at: 5 },
        end: recto_index::select::Place { page: 6, at: 7 },
    };
    reader.reveal_range(near);
    assert!(!reader.take_ops().iter().any(|op| matches!(op, Op::ScrollTo { .. })), "it's in view already");
}

/// Failure: a tile for the wrong place, or none at all, reaches the screen, because the grid and the view don't agree. Every visible cell must be covered when it says it's settled, and every attached tile must be in view.
#[test]
fn it_settles_with_what_is_visible_on_layers() {
    let mut reader = reader_on(10);
    settle(&mut reader);
    assert!(reader.is_settled(), "{}", reader.report().0);
    let report = reader.report().0;
    assert!(report["tiles"]["attached"].as_u64().unwrap() >= 1);
    assert_eq!(report["visible_pages"][0], 1);
}

/// Failure: scrolling through a long document keeps every tile on a layer, so memory grows with what's been read. Off-screen tiles must leave the layers and go discardable. Measured in surfaces, by the count on layers.
#[test]
fn scrolling_keeps_only_whats_visible_on_layers() {
    let mut reader = reader_on(400);
    settle(&mut reader);
    let first = reader.report().0["tiles"]["attached"].as_u64().unwrap();
    for step in 1..60 {
        reader.set_viewport(view(f64::from(step) * 900.0));
        settle(&mut reader);
    }
    let later = reader.report().0;
    assert!(later["tiles"]["attached"].as_u64().unwrap() <= first * 2, "{later}");
    assert!(later["tiles"]["kept_bytes"].as_u64().unwrap() > 0, "off-screen tiles stay cached");
    assert!(later["tiles"]["page_layers"].as_u64().unwrap() <= 3, "{later}");
}

/// Failure: coming back to a page drawn a moment ago shows nothing and draws it again, so a seen page goes blank. Holds the budget's "0 blank frames on seen pages", and proves the cache is really used.
#[test]
fn a_page_seen_before_comes_back_without_being_drawn() {
    let mut reader = reader_on(100);
    settle(&mut reader);
    reader.set_viewport(view(5000.0));
    settle(&mut reader);
    let before = reader.counts().drawn;
    let ops = reader.set_viewport(view(0.0));
    assert!(ops.iter().any(|op| matches!(op, Op::Attach { .. })), "something shows at once");
    assert_eq!(reader.counts().blank_seen_steps, 0);
    settle(&mut reader);
    assert_eq!(reader.counts().drawn, before, "nothing needed drawing again");
}

/// Failure: a sweep over pages already drawn has a step with a page in view and nothing on it. Measured by `qa sweep 6` on `rfc9110` from page 64: 1 blank step on a seen page in 2 of 6 sweeps (2026-11-08), then in 2 of 21 here, each time a page whose foot was in view by 0.004 points, inside the last part of a pixel that a page's tiles leave off (`device_size` rounds a page's edge to whole pixels). No tile reaches there, so refreshing took everything off the page, and the count called it bare though nothing of it could be seen. Pages are sized so their foot is such a part of a pixel, the view lands there on each page, and it sweeps at 6 pages a second over the seen pages, three times.
#[test]
fn a_sweep_over_seen_pages_has_no_blank_step() {
    // A page height whose foot, at the tiles' scale, falls in the first half of a pixel, which rounding leaves without a tile.
    let scale = {
        let mut reader = reader_on(2);
        settle(&mut reader);
        f64::from(reader.doc().unwrap().tile_scale)
    };
    let height = (0..400u16)
        .map(|step| 780.0 + f32::from(step) * 0.1)
        .find(|height| {
            let fraction = (f64::from(*height) * scale).fract();
            fraction > 0.1 && fraction < 0.4
        })
        .unwrap();
    let size = PageBox { width: 612.0, height };
    let mut reader = reader_on(40);
    settle_all_sized(&mut reader, size);
    let (step, bottoms) = {
        let doc = reader.doc().unwrap();
        let bottoms: Vec<f64> = (0..30).map(|page| doc.layout.rect(page).y + doc.layout.rect(page).height).collect();
        (doc.layout.rect(1).height * 6.0 / 120.0, bottoms)
    };
    // Down once, drawing every page passed, so every one is seen.
    let mut y = 0.0;
    while y < bottoms[25] {
        reader.set_viewport(view(y));
        settle_all_sized(&mut reader, size);
        y += step;
    }
    let blank_before = reader.counts().blank_seen_steps;
    for round in 0..3 {
        // Back up at the sweep's pace, applying as a host does and drawing nothing new.
        let mut y = bottoms[25];
        while y > 0.0 {
            reader.set_viewport(view(y));
            reader.applied();
            y -= step;
        }
        // Down again, the view's top a sliver above each page's foot, short of a pixel, and then a step on.
        for &bottom in &bottoms[..25] {
            reader.set_viewport(view(bottom - 0.004));
            reader.applied();
            reader.set_viewport(view(bottom - 0.004 + step / 2.0));
            reader.applied();
        }
        assert_eq!(reader.counts().blank_seen_steps, blank_before, "round {round}: {}", reader.report().0);
    }
    // Every page in view with a pixel of it showing has something on it, so the count is about what can be seen.
    for &bottom in &bottoms[..25] {
        for into in [0.004, 0.2, 1.0, 3.0] {
            reader.set_viewport(view(bottom - into));
            reader.applied();
            let doc = reader.doc().unwrap();
            for page in reader.visible_pages(doc) {
                let shows = !matches!(reader.visible_cells(doc, page, doc.tile_scale), Ok(cells) if cells.is_empty());
                assert!(
                    !shows || reader.attached.get(&page).is_some_and(|ids| !ids.is_empty()),
                    "page {page} is bare, {into} points in view"
                );
            }
        }
    }
}

/// Failure: coming back to two pages at once, the first takes the whole turn's attachments, and the second is blank until the next turn. Measured on `qa sweep`: 32 such steps in 5 s at 40 pages a second.
#[test]
fn every_visible_page_shows_something_in_the_first_turn() {
    let mut reader = reader_on(100);
    let tall = |y: f64| Viewport { height: 1700.0, ..view(y) };
    reader.set_viewport(tall(0.0));
    settle(&mut reader);
    reader.set_viewport(tall(30_000.0));
    settle(&mut reader);
    let ops = reader.set_viewport(tall(0.0));
    let pages: HashSet<u32> = ops
        .iter()
        .filter_map(|op| if let Op::Attach { page, .. } = op { Some(*page) } else { None })
        .collect();
    let visible = reader.report().0["visible_pages"].as_array().unwrap().len();
    assert!(visible >= 2, "this needs two pages in view");
    assert_eq!(pages.len(), visible, "{ops:?}");
    assert_eq!(reader.counts().blank_seen_steps, 0);
}

/// Failure: reading on through a long document pushes out what it just read, and a page's preview goes with its tiles, so coming back to it shows nothing at all. Measured at 40 pages a second on `rfc9110`: 84 blank steps on seen pages, all from this. Past the tile cap, tiles go and previews stay.
#[test]
fn previews_outlive_their_tiles() {
    let mut reader = reader_on(600);
    let step = reader.doc().map(|doc| doc.layout.top(1)).unwrap();
    settle(&mut reader);
    // Enough pages at 12 tiles each to pass the tile cap by a long way, and few enough to stay under the preview cap.
    for page in 1..60 {
        reader.set_viewport(view(f64::from(page) * step));
        settle(&mut reader);
    }
    let report = reader.report().0;
    assert!(
        report["tiles"]["kept_bytes"].as_u64().unwrap() <= (MAX_KEPT_BYTES + MAX_KEPT_PREVIEWS * 1024 * 1024) as u64,
        "{report}"
    );
    let doc = reader.doc().unwrap();
    let key = reader.preview_key(doc, 0);
    assert!(reader.cover.contains_key(&key), "the first page's preview is still here");
    let ops = reader.set_viewport(view(0.0));
    assert!(ops.iter().any(|op| matches!(op, Op::Attach { below: true, .. })), "and shows at once: {ops:?}");
    assert_eq!(reader.counts().blank_seen_steps, 0);
}

/// Failure: a burst of attachments stalls the UI thread, or the leftovers are forgotten and part of a page stays blank until something else happens.
#[test]
fn at_most_eight_tiles_are_attached_in_a_turn_and_the_rest_follow() {
    let mut reader = reader_on(100);
    let big = Viewport {
        width: 4000.0,
        height: 3000.0,
        frame_width: 1168.0,
        ..view(0.0)
    };
    reader.set_viewport(big);
    settle(&mut reader);
    reader.set_viewport(view(40_000.0));
    settle(&mut reader);
    let ops = reader.set_viewport(big);
    reader.applied();
    let attaches = |ops: &[Op]| ops.iter().filter(|op| matches!(op, Op::Attach { .. })).count();
    assert_eq!(attaches(&ops), MAX_ATTACH_PER_TURN, "a full turn, or this tests nothing");
    assert!(reader.has_ops(), "and it says there's more");
    let mut turns = 0;
    while reader.has_ops() {
        let more = reader.drain(MAX_ATTACH_PER_TURN);
        reader.applied();
        assert!(attaches(&more) <= MAX_ATTACH_PER_TURN);
        turns += 1;
        assert!(turns < 100, "the leftovers never end");
    }
    assert!(turns >= 1);
    assert!(reader.is_settled(), "the leftovers all came: {}", reader.report().0);
}

/// Failure: a tile from before a zoom stays on screen once its replacements are all there, or the old tiles go before the new ones arrive, leaving a blank.
#[test]
fn a_zoom_keeps_the_old_tiles_until_the_new_ones_cover_them() {
    let mut reader = reader_on(10);
    settle(&mut reader);
    let before = reader.report().0["tile_scale"].as_f64().unwrap();
    let ops = reader.set_viewport(Viewport {
        magnification: 1.5,
        ..view(0.0)
    });
    assert!(
        !ops.iter().any(|op| matches!(op, Op::Detach { .. } | Op::HidePage { .. })),
        "nothing comes off yet: {ops:?}"
    );
    settle(&mut reader);
    let after = reader.report().0;
    assert!(after["tile_scale"].as_f64().unwrap() > before);
    assert!(reader.is_settled());
    let attached = after["tiles"]["attached"].as_u64().unwrap();
    let scale = after["tile_scale"].as_f64().unwrap() as f32;
    let exact = reader
        .attached
        .values()
        .flatten()
        .filter_map(|id| reader.entries.get(id))
        .filter(|entry| entry.scale == scale)
        .count() as u64;
    assert_eq!(attached, exact, "only exact tiles left");
}

/// Failure: a pinch draws at every scale it passes through, so it stutters and throws work away.
#[test]
fn nothing_is_redrawn_during_a_pinch() {
    let mut reader = reader_on(10);
    settle(&mut reader);
    let scale = reader.report().0["tile_scale"].clone();
    reader.set_viewport(Viewport {
        magnification: 1.7,
        live: true,
        ..view(0.0)
    });
    assert_eq!(reader.report().0["tile_scale"], scale);
    assert!(reader.next_job().is_none());
}

fn a_draw_job(reader: &mut Reader, what: DrawKind) -> Job {
    loop {
        let job = reader.next_job().expect("a job");
        match &job.kind {
            JobKind::Size => reader.finish(job, Outcome::Size(letter())),
            JobKind::Text { .. } => panic!("no text was asked for"),
            JobKind::Links => reader.finish(job, Outcome::Links(None)),
            JobKind::Traits => reader.finish(job, Outcome::Heavy(false)),
            JobKind::Draw { what: kind, .. } if *kind == what => return job,
            JobKind::Draw { .. } => reader.finish(
                job,
                Outcome::Drawn {
                    kind: TileKind::Colour,
                    low_res: None,
                },
            ),
        }
    }
}

/// Failure: a job for a place the reader has left runs to the end, so a fast scroll on a heavy page queues seconds of work. It's told to stop, once, and not while it's still in view.
#[test]
fn a_job_nobody_wants_any_more_is_cancelled() {
    let mut reader = reader_on(100);
    let job = a_draw_job(&mut reader, DrawKind::Tile);
    reader.note_started(&job, None);
    reader.set_viewport(view(0.0));
    assert_eq!(reader.counts().cancels_asked, 0, "still in view");
    reader.set_viewport(view(40_000.0));
    assert_eq!(reader.counts().cancels_asked, 1, "the reader left");
    reader.set_viewport(view(41_000.0));
    assert_eq!(reader.counts().cancels_asked, 1, "told once");
    reader.finish(job, Outcome::Cancelled);
}

/// Failure: a tile being drawn for the zoom before this one runs to the end, on a heavy page for seconds, since its scale not being the view's used to count as wanted, to spare previews. It stops, and a preview, whose scale never is the view's, doesn't.
#[test]
fn a_job_for_an_old_zoom_is_cancelled_and_a_preview_is_not() {
    let mut reader = reader_on(100);
    let job = a_draw_job(&mut reader, DrawKind::Tile);
    reader.note_started(&job, None);
    reader.set_viewport(Viewport {
        magnification: 1.5,
        ..view(0.0)
    });
    assert_eq!(reader.counts().cancels_asked, 1, "the zoom moved on");
    reader.finish(job, Outcome::Cancelled);

    let mut reader = reader_on(100);
    let job = a_draw_job(&mut reader, DrawKind::Preview);
    reader.note_started(&job, None);
    reader.set_viewport(Viewport {
        magnification: 1.5,
        ..view(0.0)
    });
    assert_eq!(reader.counts().cancels_asked, 0, "its page is still in view");
    reader.finish(job, Outcome::Cancelled);
}

/// Failure: a cold tab keeps its tiles, layout or renderer's file. Budget: 0 tiles and 0 handles for a cold tab.
#[test]
fn a_tab_gone_cold_holds_nothing() {
    let mut reader = reader_on(10);
    settle(&mut reader);
    assert!(!reader.entries.is_empty());
    reader.activate(
        2,
        10,
        0,
        PageMap {
            files: vec![PathBuf::from("/d/b.pdf")],
            slots: None,
            own: None,
            turns: None,
            expected: Vec::new(),
            follows: None,
        },
        &[],
        None,
        Spread::Single,
    );
    assert!(reader.entries.values().all(|entry| entry.tab != 1), "tab 1 isn't warm");
    assert!(!reader.docs.contains_key(&1));
    assert_eq!(*reader.wanted_files(), vec![PathBuf::from("/d/b.pdf")]);
    reader.activate(
        3,
        10,
        0,
        PageMap {
            files: vec![PathBuf::from("/d/c.pdf")],
            slots: None,
            own: None,
            turns: None,
            expected: Vec::new(),
            follows: None,
        },
        &[2],
        None,
        Spread::Single,
    );
    assert_eq!(reader.wanted_files().len(), 2, "a warm tab's file is kept");
}

/// Failure: the window closed still holds tiles, so a closed window stays over budget.
#[test]
fn a_closed_window_holds_no_tiles_and_comes_back() {
    let mut reader = reader_on(10);
    settle(&mut reader);
    reader.window_closed();
    assert!(reader.entries.is_empty());
    assert!(reader.next_job().is_none(), "nothing draws for a closed window");
    assert!(reader.wanted_files().is_empty(), "the renderer goes too");
    reader.window_opened();
    reader.set_viewport(view(0.0));
    settle(&mut reader);
    assert!(reader.is_settled());
    assert!(!reader.entries.is_empty());
}

/// Failure: dark paper shows white tiles, or the paper's tiles are thrown away on a change so going back draws everything again.
#[test]
fn a_change_of_paper_draws_for_that_paper_and_keeps_the_other() {
    let mut reader = reader_on(10);
    settle(&mut reader);
    let before = reader.counts().drawn;
    reader.set_paper(PaperKind::Dark);
    let drawn = settle(&mut reader);
    assert!(drawn.iter().any(|job| matches!(job.kind, JobKind::Draw { paper: PaperKind::Dark, .. })));
    assert!(reader.counts().drawn > before);
    let dark = reader.counts().drawn;
    reader.set_paper(PaperKind::White);
    settle(&mut reader);
    assert_eq!(reader.counts().drawn, dark, "back again with nothing to draw");
}

/// Failure: an agent's `goto` to a page reads as the reader having scrolled there and then scrolled back, so the session's place fights the jump. Arriving isn't a move, and a real scroll afterwards is.
#[test]
fn a_jump_isnt_reported_back_as_a_move() {
    let mut reader = reader_on(100);
    reader.scrolled_to = 0;
    settle(&mut reader);
    let mut base = view(0.0);
    base.scrolled_to = 0;
    reader.set_viewport(base);
    reader.take_moved_page();
    reader.jump_to(50);
    let Some(Op::ScrollTo { y, seq }) = reader.take_ops().into_iter().find(|op| matches!(op, Op::ScrollTo { .. })) else {
        panic!("no scroll")
    };
    let mut there = view(y);
    there.scrolled_to = seq - 1;
    reader.set_viewport(there);
    assert_eq!(reader.take_moved_page(), None, "not applied yet");
    there.scrolled_to = seq;
    reader.set_viewport(there);
    assert_eq!(reader.take_moved_page(), None, "arrived");
    there.y = y + 5000.0;
    reader.set_viewport(there);
    assert!(reader.take_moved_page().is_some_and(|page| page > 50), "the reader moved");
}

/// Failure (slow mode, 2026-11-11: five update tests): the host puts the view on its own pixels, so it arrives a fraction of a point off where it was sent, and the next word from it that isn't the arrival (the window shown, a layout's tick, a pixel's resize) was taken for the reader moving. The place was taken again from the rounded height: the top of page 9 became the foot of page 8 (it was in the gap above), or a ten-thousandth into the page, and the next launch went there. A view within a point of where the place says is still on the place, said the host's way.
#[test]
fn a_view_the_host_rounded_to_its_pixels_is_still_on_the_place() {
    let mut reader = reader_on(100);
    reader.scrolled_to = 0;
    settle(&mut reader);
    let mut base = view(0.0);
    base.scrolled_to = 0;
    reader.set_viewport(base);
    reader.take_moved_page();
    reader.jump_to(8);
    let Some(Op::ScrollTo { y, seq }) = reader.take_ops().into_iter().find(|op| matches!(op, Op::ScrollTo { .. })) else {
        panic!("no scroll")
    };
    let anchor = reader.doc().unwrap().layout.anchor_for(y, 8);
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
        assert_eq!(reader.take_moved_page(), None, "and it isn't a move");
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
    reader.scrolled_to = 0;
    settle(&mut reader);
    let mut base = view(0.0);
    base.scrolled_to = 0;
    reader.set_viewport(base);
    reader.take_user_scrolled();
    let mut scrolled = base;
    scrolled.y = 2000.0;
    reader.set_viewport(scrolled);
    assert!(reader.take_user_scrolled(), "the reader scrolled");
    assert!(!reader.take_user_scrolled(), "asking clears it");
    let page = reader.follow(40, 0.25).expect("a tab is showing");
    assert_eq!(page, 40);
    let Some(Op::ScrollTo { y, seq }) = reader.take_ops().into_iter().rfind(|op| matches!(op, Op::ScrollTo { .. })) else {
        panic!("no scroll")
    };
    let mut there = scrolled;
    there.y = y;
    there.scrolled_to = seq;
    reader.set_viewport(there);
    assert!(!reader.take_user_scrolled(), "arriving isn't leading");
    assert_eq!(reader.reading_anchor().map(|(page, _)| page), Some(40));
    let mut zoomed = there;
    zoomed.magnification = 1.5;
    zoomed.y += 300.0;
    reader.set_viewport(zoomed);
    assert!(!reader.take_user_scrolled(), "a zoom isn't a scroll");
    let mut pinch = zoomed;
    pinch.live = true;
    pinch.y += 50.0;
    reader.set_viewport(pinch);
    assert!(!reader.take_user_scrolled(), "nor is a pinch");
    assert_eq!(reader.follow(10_000, 2.0), Some(99), "held to the document");
}

/// Failure (found by restore's whole-state test, with comparison): a view that arrives where it was sent shows another page where the reader's eye is, as a narrower pane does, and the page view's place says one page while the session says another, so the place isn't brought back after a restart. Arriving on another page than was asked says so.
#[test]
fn arriving_on_another_page_than_asked_says_so() {
    let mut reader = reader_on(100);
    reader.scrolled_to = 0;
    settle(&mut reader);
    reader.take_moved_page();
    assert_eq!(reader.follow(40, 0.9), Some(40));
    assert_eq!(reader.take_moved_page(), Some(40), "sent there");
    let Some(Op::ScrollTo { y, seq }) = reader.take_ops().into_iter().rfind(|op| matches!(op, Op::ScrollTo { .. })) else {
        panic!("no scroll")
    };
    let mut there = view(y);
    there.scrolled_to = seq;
    reader.set_viewport(there);
    assert_eq!(reader.take_moved_page(), Some(41), "a third of the way down the view is the next page");
    let mut again = there;
    again.y += 1.0;
    reader.set_viewport(again);
    assert_eq!(reader.take_moved_page(), None, "and nothing more until it moves on");
}

/// Failure: three panes of a comparison keep three times what one view keeps off screen. Shared, each keeps its share, and what's over goes at once.
#[test]
fn panes_share_the_caps_on_what_is_kept() {
    let mut reader = reader_on(400);
    settle(&mut reader);
    for step in 0..60 {
        reader.set_viewport(view(f64::from(step) * 4000.0));
        settle(&mut reader);
    }
    let previews = |reader: &Reader| reader.entries.values().filter(|entry| entry.preview && !entry.attached).count();
    assert!(previews(&reader) > MAX_KEPT_PREVIEWS / 3, "enough kept to see the share: {}", previews(&reader));
    reader.set_share(3);
    assert!(previews(&reader) <= MAX_KEPT_PREVIEWS / 3, "{}", previews(&reader));
    reader.set_share(0);
    assert_eq!(reader.share, 1, "never none");
}

/// Failure (found by restore's whole-state test): side by side, going to a right-hand page arrives at a view whose top row starts with the left one, so the page view's place says the left page while the session says the right, and the place isn't brought back after a restart. And a change of layout moves the place to the other page of a row.
#[test]
fn side_by_side_keeps_the_page_of_its_row_the_reader_was_on() {
    let mut reader = reader_on(100);
    reader.scrolled_to = 0;
    reader.set_spread(Spread::TwoUp);
    settle(&mut reader);
    reader.take_moved_page();
    reader.jump_to(51);
    let Some(Op::ScrollTo { y, seq }) = reader.take_ops().into_iter().rfind(|op| matches!(op, Op::ScrollTo { .. })) else {
        panic!("no scroll")
    };
    let mut there = view(y);
    there.scrolled_to = seq;
    reader.set_viewport(there);
    assert_eq!(reader.take_moved_page(), None, "arrived");
    assert_eq!(reader.places()[0].1.page, 51, "the right-hand page it went to");
    there.y = y + 20.0;
    reader.set_viewport(there);
    assert_eq!(reader.take_moved_page(), None, "reading on in the row isn't going to the left page");
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
            reader.scrolled_to = 0;
            reader.set_spread(Spread::TwoUp);
            settle(&mut reader);
            reader.jump_to(target);
            let mut at = follow_host(&mut reader, view(0.0));
            reader.take_moved_page();
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
                assert_eq!(reader.reading_anchor().map(|(page, _)| page), Some(target));
                assert!(reader.take_moved_page().is_none_or(|page| page == target), "the session's page left {target}");
            }
        }
    }
}

/// Failure (PLAN, side-by-side follow-ups): the host is told the document is as wide as a fold-out wherever the view is, leaving room to scroll across beside every other page; or it isn't told it's wider when the fold-out comes into view, so its right side can't be reached.
#[test]
fn the_host_hears_the_documents_width_for_what_it_shows() {
    let mut reader = reader_on(40);
    settle(&mut reader);
    let doc = reader.docs.get_mut(&1).unwrap();
    doc.layout.set_page_size(21, PageBox { width: 3000.0, height: 792.0 });
    let (wide, frame) = (doc.layout.rect(21), doc.layout.document_width_for(0.0, 1.0));
    reader.lay_out();
    reader.take_ops();
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

/// The view the host goes to on the reader's next turn: wherever the last scroll it was told of says, as it would have applied it.
fn follow_host(reader: &mut Reader, mut at: Viewport) -> Viewport {
    if let Some(Op::ScrollTo { y, seq }) = reader.take_ops().into_iter().rfind(|op| matches!(op, Op::ScrollTo { .. })) {
        at.y = y;
        at.scrolled_to = seq;
    }
    reader.set_viewport(at);
    at
}

/// Failure (review of f05c998, finding 5): side by side, a sleep file's pictures cover half as many rows of a view as a page to a row does, so waking a tab, or an update's first frame, shows bare pages where it didn't before. Every page of the view at a zoom that shows six rows of pairs gets one.
#[test]
fn a_sleep_file_covers_the_view_side_by_side_too() {
    let mut reader = reader_on(40);
    reader.note_version(Path::new("/d/a.pdf"), 1);
    reader.set_spread(Spread::TwoUp);
    let mut far = view(0.0);
    far.height = 2400.0;
    reader.set_viewport(far);
    settle(&mut reader);
    let shown: Vec<u32> = reader.visible_pages(reader.doc().unwrap()).collect();
    assert!(shown.len() > 6 && shown.len() <= 12, "the view shows {} pages", shown.len());
    let pictures: Vec<u32> = reader.sleep_material(1, None).unwrap().pictures.iter().map(|picture| picture.page).collect();
    assert_eq!(pictures, shown, "a picture for each page shown");
}

/// Failure (second review): a tab off screen gets its sleep pictures for the height of the tab showing's view at that tab's zoom, not its own, so a tab left side by side at a low zoom got 4 pictures of the 12 pages it showed, and woke with bare pages.
#[test]
fn a_tab_off_screen_gets_pictures_for_its_own_view() {
    let mut reader = reader_on(40);
    reader.note_version(Path::new("/d/a.pdf"), 1);
    reader.scrolled_to = 0;
    reader.set_spread(Spread::TwoUp);
    let mut low = view(0.0);
    low.magnification = 0.4;
    low.width /= 0.4;
    low.height /= 0.4;
    reader.set_viewport(low);
    settle(&mut reader);
    let shown: Vec<u32> = reader.visible_pages(reader.doc().unwrap()).take(12).collect();
    assert!(shown.len() > 6, "the low zoom shows {} pages", shown.len());
    // Another tab comes to the screen at its own zoom, and the first is held, off screen.
    reader.activate(
        2,
        10,
        0,
        PageMap {
            files: vec![PathBuf::from("/d/b.pdf")],
            ..map()
        },
        &[1],
        None,
        Spread::Single,
    );
    reader.set_viewport(view(0.0));
    settle(&mut reader);
    let pictures: Vec<u32> = reader.sleep_material(1, None).unwrap().pictures.iter().map(|picture| picture.page).collect();
    assert_eq!(pictures, shown, "a picture for each page it showed");
}

/// Failure (full check, 2026-11-08, `a_new_version_of_a_file_has_none_of_the_old_ones_pictures`): a tab left for another has its previews and tiles marked discardable, and the system takes that memory back whenever it likes, with no word to the app. Put to sleep after that, its sleep file had no picture at all, and it woke to bare paper. What it showed when it was left is what it sleeps with, whatever the system has taken since.
#[test]
fn a_tab_left_keeps_its_pictures_when_the_system_takes_its_tiles() {
    let mut reader = reader_on(40);
    reader.note_version(Path::new("/d/a.pdf"), 1);
    settle(&mut reader);
    let shown: Vec<u32> = reader.visible_pages(reader.doc().unwrap()).take(MAX_SLEEP_SHOTS).collect();
    reader.activate(
        2,
        10,
        0,
        PageMap {
            files: vec![PathBuf::from("/d/b.pdf")],
            ..map()
        },
        &[1],
        None,
        Spread::Single,
    );
    settle(&mut reader);
    assert!(
        reader
            .entries
            .values()
            .any(|entry| entry.tab == 1 && entry.discardable && entry.surface.is_some()),
        "what the tab left showed is discardable now"
    );
    // What the kernel does to discardable memory when it needs pages.
    assert!(reader.empty_discardable() > 0);
    assert!(
        reader
            .entries
            .values()
            .filter(|entry| entry.tab == 1 && entry.discardable)
            .filter_map(|entry| entry.surface.as_ref())
            .all(PixelSurface::was_emptied)
    );
    let pictures: Vec<u32> = reader.sleep_material(1, None).unwrap().pictures.iter().map(|picture| picture.page).collect();
    assert_eq!(pictures, shown, "a picture for each page it showed when it was left");
    // Shown again, it's drawn again, and what was kept for its sleep file goes.
    reader.activate(1, 40, 0, map(), &[2], None, Spread::Single);
    assert!(reader.docs[&1].left_pictures.is_none());
    assert_eq!(reader.report().0["left_picture_bytes"], 0);
}

/// Failure: a tab left showing one version of its file keeps its pictures after a worker says the file is another version now, so its sleep file shows the old pages.
#[test]
fn pictures_kept_of_a_tab_left_go_with_its_version() {
    let mut reader = reader_on(10);
    reader.note_version(Path::new("/d/a.pdf"), 1);
    settle(&mut reader);
    reader.activate(
        2,
        10,
        0,
        PageMap {
            files: vec![PathBuf::from("/d/b.pdf")],
            ..map()
        },
        &[1],
        None,
        Spread::Single,
    );
    settle(&mut reader);
    let kept = reader.report().0["left_picture_bytes"].as_u64().unwrap();
    assert!(
        kept > 0 && kept <= MAX_LEFT_PICTURE_BYTES as u64,
        "kept as it was left, within its limit: {kept}"
    );
    reader.note_version(Path::new("/d/a.pdf"), 2);
    assert!(reader.docs[&1].left_pictures.is_none());
    assert!(
        reader.sleep_material(1, None).unwrap().pictures.is_empty(),
        "nothing of version 1 is left to give"
    );
}

/// Failure (review of f05c998, finding 1): Rotate, Undo or Redo of a turn sends the reader to the top of their page and back to the left edge, as the tab showing was taken up again as if it had just been come to.
#[test]
fn a_turn_keeps_the_place_in_the_page_and_across() {
    let mut reader = reader_on(10);
    reader.scrolled_to = 0;
    settle(&mut reader);
    let page = reader.doc().unwrap().layout.rect(3);
    let mut at = view(page.y + page.height * 0.4);
    at.x = 30.0;
    at.magnification = 2.0;
    reader.set_viewport(at);
    reader.take_moved_page();
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
    assert_eq!(reader.docs[&1].scroll_x, 30.0, "and as far across");
}

/// Answers what the reader asks as a file whose page 0 is `first` and the rest letter would, until it asks nothing more.
fn settle_sized(reader: &mut Reader, first: PageBox) {
    for _ in 0..10_000 {
        reader.take_ops();
        reader.applied();
        let Some(job) = reader.next_job() else { break };
        let outcome = match &job.kind {
            JobKind::Size => Outcome::Size(if job.position == 0 { first } else { letter() }),
            JobKind::Text { .. } => Outcome::TextFailed,
            JobKind::Links => Outcome::Links(None),
            JobKind::Traits => Outcome::Heavy(false),
            JobKind::Draw { .. } => Outcome::Drawn {
                kind: TileKind::Colour,
                low_res: None,
            },
        };
        reader.finish(job, outcome);
        drop(reader.drain(MAX_ATTACH_PER_TURN));
    }
}

/// Failure (review round 1 of stage 2): an added file's stamp is looked up by path across every tab, so B.pdf added into tab A and open as tab B's own: saving B makes tab B's own pages fail as "a PDF that has changed since it was added". Only the tab the file was added to checks it, and never for its own file.
#[test]
fn an_added_files_stamp_is_the_tabs_it_was_added_to() {
    let stamp = Some(FileStamp {
        size: 1,
        mtime_ns: 1,
        device: 0,
        inode: 3,
    });
    let mut reader = Reader::new();
    reader.activate(
        1,
        2,
        0,
        PageMap {
            files: vec![PathBuf::from("/d/a.pdf"), PathBuf::from("/d/b.pdf")],
            slots: Some(vec![(0, 0), (1, 0)]),
            expected: vec![None, stamp],
            ..map()
        },
        &[],
        None,
        Spread::Single,
    );
    reader.activate(
        2,
        1,
        0,
        PageMap {
            files: vec![PathBuf::from("/d/b.pdf")],
            ..map()
        },
        &[1],
        None,
        Spread::Single,
    );
    assert_eq!(reader.expected_stamp(1, Path::new("/d/b.pdf")), stamp, "tab 1 added it");
    assert_eq!(reader.expected_stamp(2, Path::new("/d/b.pdf")), None, "it's tab 2's own file");
    assert_eq!(reader.expected_stamp(1, Path::new("/d/a.pdf")), None, "and tab 1's own isn't checked so");
}

/// Failure (the owner's bench file, 2026-11-08): a save after turning page 1 changes the view. The saved file's page 1 is landscape, the reopened layout fits that page instead of the portrait ones, and the zoom shown drops from 128% to 97% with every page shrinking; the place within the page and the selection go too. A save writes what was shown, so the fit, zoom, place across and down, layout and selection stay exactly, whichever page the reader's on and whether a renderer saw the file replaced first.
#[test]
fn a_save_leaves_zoom_place_layout_and_selection_as_they_were() {
    let stamp = |size: u64| {
        Some(FileStamp {
            size,
            mtime_ns: 1,
            device: 0,
            inode: 7,
        })
    };
    let landscape = PageBox { width: 792.0, height: 612.0 };
    // On the turned page itself, as on the bench file, the first page the new layout learns is the landscape one.
    for (spread, renderer_saw_it_first, on) in [
        (Spread::Single, false, 0),
        (Spread::Single, false, 3),
        (Spread::Single, true, 3),
        (Spread::TwoUpCover, false, 3),
    ] {
        let mut reader = Reader::new();
        reader.set_viewport(view(0.0));
        let own = PageMap { own: stamp(1), ..map() };
        reader.activate(1, 10, 0, own.clone(), &[], None, spread);
        reader.scrolled_to = 0;
        let mut turns = vec![0u8; 10];
        turns[0] = 1;
        let turned = PageMap {
            turns: Some(Arc::from(turns)),
            ..own.clone()
        };
        reader.activate(1, 10, 0, turned, &[], None, spread);
        follow_host(&mut reader, view(0.0));
        settle_sized(&mut reader, letter());
        reader.zoom(Zoom::To(1.28));
        let page = reader.doc().unwrap().layout.rect(on);
        let mut at = view(page.y + page.height * 0.4);
        at.x = 25.0;
        at.magnification = 1.28;
        reader.set_viewport(at);
        reader.take_moved_page();
        let selected = Range {
            start: recto_index::select::Place { page: 2, at: 5 },
            end: recto_index::select::Place { page: 4, at: 9 },
        };
        reader.restore_selection(selected);
        let (fit, height, width, zoom) = {
            let doc = reader.doc().unwrap();
            (doc.layout.fit(), doc.layout.document_height(), doc.layout.document_width(), doc.magnification)
        };
        let rects: Vec<crate::layout::PageRect> = (0..10).map(|page| reader.doc().unwrap().layout.rect(page)).collect();
        let before = reader.places()[0].1;
        if renderer_saw_it_first {
            reader.file_changed(1);
        }
        // Saved: the file is another version, with page 0 turned in it, and no edits left.
        let saved = PageMap {
            files: own.files.clone(),
            slots: None,
            own: stamp(2),
            turns: None,
            expected: Vec::new(),
            follows: stamp(1),
        };
        reader.activate(1, 10, on, saved, &[], None, spread);
        follow_host(&mut reader, at);
        settle_sized(&mut reader, landscape);
        let doc = reader.doc().unwrap();
        let label = format!("{spread:?}, renderer first: {renderer_saw_it_first}, on page {on}");
        assert_eq!(doc.layout.fit(), fit, "{label}: the fit, and so the zoom shown");
        assert_eq!(doc.magnification, zoom, "{label}: the magnification");
        assert_eq!(
            (doc.layout.document_height(), doc.layout.document_width()),
            (height, width),
            "{label}: the document's size"
        );
        assert_eq!(
            (0..10).map(|page| doc.layout.rect(page)).collect::<Vec<_>>(),
            rects,
            "{label}: every page where it was"
        );
        let after = reader.places()[0].1;
        assert_eq!(after.page, before.page, "{label}: {before:?} then {after:?}");
        assert!(
            (after.anchor.1 - before.anchor.1).abs() < 1e-9 && (after.y - before.y).abs() < 1e-6,
            "{label}: as far down: {before:?} then {after:?}"
        );
        assert_eq!(after.x, before.x, "{label}: as far across");
        assert_eq!(reader.selection_range(), Some(selected), "{label}: what was selected");
    }
    // Any other new version is other pages: nothing's carried.
    let mut reader = reader_on(10);
    settle_sized(&mut reader, letter());
    reader.restore_selection(Range::pages(1, 2));
    reader.activate(
        1,
        10,
        0,
        PageMap {
            own: stamp(9),
            follows: stamp(8),
            ..map()
        },
        &[],
        None,
        Spread::Single,
    );
    assert_eq!(reader.selection_range(), None, "a save from another version than the one shown carries nothing");
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
        reader.scrolled_to = 0;
        reader.set_spread(from);
        settle(&mut reader);
        reader.jump_to(51);
        let at = follow_host(&mut reader, view(0.0));
        let mut scrolled = at;
        scrolled.y += 20.0;
        reader.set_viewport(scrolled);
        reader.take_moved_page();
        let was = reader.doc().unwrap().layout.rect(51);
        let down = (scrolled.y - was.y) / was.height;
        reader.set_spread(to);
        follow_host(&mut reader, scrolled);
        let place = reader.places()[0].1;
        assert_eq!(place.page, 51, "{from:?} to {to:?}");
        let rect = reader.doc().unwrap().layout.rect(51);
        assert!(
            (reader.viewport.y - (rect.y + down * rect.height)).abs() < 1.0,
            "{from:?} to {to:?}: as far down the page: {} against {}",
            reader.viewport.y,
            rect.y + down * rect.height
        );
    }
}

/// Failure: a page that couldn't be drawn is asked for on every scroll, or holds up the rest, or shows no reason.
#[test]
fn a_failed_page_says_why_and_isnt_asked_for_again() {
    let mut reader = reader_on(3);
    for _ in 0..16 {
        let Some(job) = reader.next_job() else { break };
        let outcome = match &job.kind {
            JobKind::Size => Outcome::Size(letter()),
            JobKind::Text { .. } => panic!("no text was asked for"),
            JobKind::Links => Outcome::Links(None),
            JobKind::Traits => Outcome::Heavy(false),
            JobKind::Draw { .. } if job.position == 0 => Outcome::PageFailed {
                message: "This page couldn't be drawn.".into(),
                may_pass: false,
            },
            JobKind::Draw { .. } => Outcome::Drawn {
                kind: TileKind::Colour,
                low_res: None,
            },
        };
        reader.finish(job, outcome);
    }
    reader.set_viewport(view(0.0));
    assert_eq!(reader.note(), Some("This page couldn't be drawn."));
    assert!(reader.next_job().is_none_or(|job| job.position != 0));
}

/// Failure: a lower-resolution page looks like a sharp one, with nothing to say so. SPEC §3: it says so. And one that may clear is tried again once its time is up.
#[test]
fn a_lower_resolution_page_is_labelled() {
    let mut reader = reader_on(1);
    let rect = TileRect {
        x: 0,
        y: 0,
        width: 100,
        height: 100,
    };
    let mut low_done = false;
    for _ in 0..64 {
        let Some(job) = reader.next_job() else { break };
        let outcome = match &job.kind {
            JobKind::Size => Outcome::Size(letter()),
            JobKind::Text { .. } => panic!("no text was asked for"),
            JobKind::Links => Outcome::Links(None),
            JobKind::Traits => Outcome::Heavy(true),
            JobKind::Draw { .. } => {
                low_done = true;
                Outcome::Drawn {
                    kind: TileKind::Colour,
                    low_res: Some((rect, LowResWhy::TooHeavy)),
                }
            }
        };
        reader.finish(job, outcome);
        reader.drain(64);
        reader.applied();
    }
    assert!(low_done);
    assert_eq!(reader.note(), Some(LowResWhy::TooHeavy.message()));
    assert_eq!(reader.report().0["low_res_pages"][0], 1);
    assert!(reader.is_settled(), "{}", reader.report().0);
}

/// Failure: a heavy page is drawn a tile at a time, each pass costing it half a second, as the first version did on the USGS map.
#[test]
fn a_heavy_page_is_drawn_in_regions_with_no_preview() {
    let mut reader = reader_on(1);
    let mut kinds = Vec::new();
    for _ in 0..64 {
        let Some(job) = reader.next_job() else { break };
        let outcome = match &job.kind {
            JobKind::Size => Outcome::Size(PageBox { width: 1224.0, height: 1584.0 }),
            JobKind::Text { .. } => panic!("no text was asked for"),
            JobKind::Links => Outcome::Links(None),
            JobKind::Traits => Outcome::Heavy(true),
            JobKind::Draw { what, .. } => {
                kinds.push(*what);
                Outcome::Drawn {
                    kind: TileKind::Colour,
                    low_res: None,
                }
            }
        };
        reader.finish(job, outcome);
        reader.drain(64);
        reader.applied();
    }
    assert!(kinds.contains(&DrawKind::Region), "{kinds:?}");
    assert!(!kinds.contains(&DrawKind::Preview), "{kinds:?}");
    assert!(reader.is_settled(), "{}", reader.report().0);
}

/// Failure: edits that reorder pages leave tiles of the old order on screen.
#[test]
fn a_change_to_the_page_order_clears_what_was_drawn() {
    let mut reader = reader_on(5);
    settle(&mut reader);
    let edited = PageMap {
        files: vec![PathBuf::from("/d/a.pdf")],
        slots: Some(vec![(0, 1), (0, 0)]),
        own: None,
        turns: None,
        expected: Vec::new(),
        follows: None,
    };
    reader.activate(1, 2, 0, edited.clone(), &[], None, Spread::Single);
    assert!(reader.entries.is_empty());
    assert_eq!(reader.doc().unwrap().layout.count(), 2);
    assert_eq!(edited.source(0), Some((PathBuf::from("/d/a.pdf"), 1)));
    assert_eq!(edited.source(9), None);
}

/// Failure: a page whose size can't be read is asked about again at once, for ever: 100% of a core on a hostile page, nothing else is drawn, and everything waiting for it to settle times out.
#[test]
fn a_page_whose_size_cant_be_read_is_asked_once() {
    let mut reader = reader_on(3);
    let mut asked = 0;
    for _ in 0..200 {
        reader.take_ops();
        reader.applied();
        let Some(job) = reader.next_job() else { break };
        let outcome = match &job.kind {
            JobKind::Size if job.position == 0 => {
                asked += 1;
                Outcome::PageFailed {
                    message: "This page couldn't be read.".into(),
                    may_pass: false,
                }
            }
            JobKind::Size => Outcome::Size(letter()),
            JobKind::Text { .. } => panic!("no text was asked for"),
            JobKind::Links => Outcome::Links(None),
            JobKind::Traits => Outcome::Heavy(false),
            JobKind::Draw { .. } => Outcome::Drawn {
                kind: TileKind::Colour,
                low_res: None,
            },
        };
        reader.finish(job, outcome);
        reader.drain(64);
        reader.applied();
    }
    assert_eq!(asked, 1);
    assert!(reader.is_settled(), "{}", reader.report().0);
    assert_eq!(reader.note(), Some("This page couldn't be read."));
}

/// Failure: a page is deleted while a job on it is running, and when the job ends its old picture goes in the place of the page now there, reading as settled. The same for a size or a heavy answer about what used to be there.
#[test]
fn what_a_job_found_before_an_edit_is_thrown_away() {
    let edited = PageMap {
        files: vec![PathBuf::from("/d/a.pdf")],
        slots: Some(vec![(0, 1), (0, 2)]),
        own: None,
        turns: None,
        expected: Vec::new(),
        follows: None,
    };
    // A job of each kind, caught running.
    let mut reader = reader_on(3);
    let size = reader.next_job().expect("a size");
    assert!(matches!(size.kind, JobKind::Size));
    reader.activate(1, 2, 0, edited.clone(), &[], None, Spread::Single);
    reader.finish(size, Outcome::Size(PageBox { width: 100.0, height: 100.0 }));
    assert!(!reader.doc().unwrap().layout.is_known(0), "that size was the old page's");

    let mut reader = reader_on(3);
    let traits = loop {
        let job = reader.next_job().expect("a job");
        match job.kind {
            JobKind::Size => reader.finish(job, Outcome::Size(letter())),
            JobKind::Text { .. } => panic!("no text was asked for"),
            JobKind::Links => reader.finish(job, Outcome::Links(None)),
            JobKind::Traits => break job,
            JobKind::Draw { .. } => panic!("traits come before drawing"),
        }
    };
    let position = traits.position;
    reader.activate(1, 2, 0, edited.clone(), &[], None, Spread::Single);
    reader.finish(traits, Outcome::Heavy(true));
    assert!(!reader.doc().unwrap().heavy.contains_key(&position), "that was the old page's");

    let mut reader = reader_on(3);
    let draw = loop {
        let job = reader.next_job().expect("a job");
        match job.kind {
            JobKind::Size => reader.finish(job, Outcome::Size(letter())),
            JobKind::Text { .. } => panic!("no text was asked for"),
            JobKind::Links => reader.finish(job, Outcome::Links(None)),
            JobKind::Traits => reader.finish(job, Outcome::Heavy(false)),
            JobKind::Draw { .. } => break job,
        }
    };
    reader.note_started(&draw, None);
    reader.activate(1, 2, 0, edited, &[], None, Spread::Single);
    reader.finish(
        draw,
        Outcome::Drawn {
            kind: TileKind::Colour,
            low_res: None,
        },
    );
    assert!(reader.entries.is_empty(), "the old page's picture stays out");
    assert!(reader.cover.is_empty());
}

/// Failure: a surface that held one file's page is handed to another file's worker, so a hostile file reads a page of the one before it. Both ways round: the next file's worker gets surfaces a cold tab's tiles had, and a hostile file that kept a reference, answered "blank" and got its surface taken back then sees the next file's pixels and can repaint them. No surface may ever serve two files. Each is held here, as a hostile worker holds it, so its name can't be given to a new one by chance.
#[test]
fn no_surface_serves_two_files() {
    fn run(reader: &mut Reader, answer: TileKind, held: &mut Vec<PixelSurface>) -> HashSet<u32> {
        let mut seen = HashSet::new();
        for _ in 0..1000 {
            reader.take_ops();
            reader.applied();
            let Some(job) = reader.next_job() else { break };
            let outcome = match &job.kind {
                JobKind::Size => Outcome::Size(letter()),
                JobKind::Text { .. } => panic!("no text was asked for"),
                JobKind::Links => Outcome::Links(None),
                JobKind::Traits => Outcome::Heavy(false),
                JobKind::Draw { surface, .. } => {
                    seen.insert(surface.id());
                    held.push(surface.clone());
                    Outcome::Drawn { kind: answer, low_res: None }
                }
            };
            reader.finish(job, outcome);
        }
        seen
    }
    let other = |name: &str| PageMap {
        files: vec![PathBuf::from(name)],
        slots: None,
        own: None,
        turns: None,
        expected: Vec::new(),
        follows: None,
    };
    for answer in [TileKind::Colour, TileKind::Blank { rgb: 0xFF_FF_FF }] {
        let mut held = Vec::new();
        let mut reader = reader_on(3);
        let first = run(&mut reader, answer, &mut held);
        reader.activate(2, 3, 0, other("/d/b.pdf"), &[], None, Spread::Single);
        reader.set_viewport(view(0.0));
        let second = run(&mut reader, TileKind::Colour, &mut held);
        assert!(!first.is_empty() && !second.is_empty());
        assert!(first.is_disjoint(&second), "served both: {:?}", first.intersection(&second).collect::<Vec<_>>());
    }
}

/// Failure: a tile comes off in Rust just after the UI thread took its ops, and "applied" for those ops marks it discardable while its layer is still showing it, so the OS can empty a surface on screen. Only what the host was told about counts.
#[test]
fn a_surface_goes_discardable_only_after_the_host_was_told() {
    let mut reader = reader_on(3);
    settle(&mut reader);
    let (&page, ids) = reader.attached.iter().next().unwrap();
    let id = ids[0];
    reader.take_ops();
    // The engine's thread, between the host's taking its ops and saying it has applied them.
    reader.detach(id, page);
    reader.applied();
    assert!(!reader.entries[&id].discardable, "the layer still has it");
    reader.take_ops();
    reader.applied();
    assert!(reader.entries[&id].discardable, "now the host has been told");
}

/// Finishes everything that isn't text, with a pretend renderer, and hands back the first text job.
fn settle_to_text(reader: &mut Reader) -> Option<Job> {
    loop {
        reader.take_ops();
        reader.applied();
        let job = reader.next_job()?;
        match &job.kind {
            JobKind::Text { .. } => return Some(job),
            JobKind::Links => reader.finish(job, Outcome::Links(None)),
            JobKind::Size => reader.finish(job, Outcome::Size(letter())),
            JobKind::Traits => reader.finish(job, Outcome::Heavy(false)),
            JobKind::Draw { .. } => reader.finish(
                job,
                Outcome::Drawn {
                    kind: TileKind::Colour,
                    low_res: None,
                },
            ),
        }
    }
}

/// Failure: a hidden-text check that takes seconds keeps the page the reader has just scrolled to blank, over and over on a hostile file, as a running text job couldn't be told to stop. Its first read, which is quick and is what the reader's hand is waiting on, isn't given up for a scroll.
#[test]
fn a_check_goes_when_the_view_wants_something_and_a_read_does_not() {
    let mut reader = reader_on(100);
    settle_to_text(&mut reader);
    reader.select_all();
    let first = settle_to_text(&mut reader).expect("what shows is read");
    assert!(matches!(first.kind, JobKind::Text { check: false }), "{first:?}");
    reader.note_started(&first, None);
    reader.set_viewport(view(40_000.0));
    assert!(reader.running_is_wanted(), "a read stays, as what's showing needs it");
    reader.running = None;
    // Back where it was, with everything there, and the check runs.
    reader.set_viewport(view(0.0));
    let text = Arc::new(recto_index::select::PageText::default());
    reader.finish(first, Outcome::Text { text, checked: false });
    let check = settle_to_text(&mut reader).expect("selected and showing, so it's checked");
    assert!(matches!(check.kind, JobKind::Text { check: true }), "{check:?}");
    reader.note_started(&check, None);
    assert!(reader.running_is_wanted(), "nothing else is wanted");
    // A scroll to pages with nothing drawn.
    let asked = reader.counts.cancels_asked;
    reader.set_viewport(view(40_000.0));
    assert!(!reader.running_is_wanted(), "the view wants tiles");
    assert_eq!(reader.counts.cancels_asked, asked + 1);
}

/// Failure: a check that was stopped reads as one that was tried, so the page is never checked, or a stopped read holds a place in the cache as a page that failed.
#[test]
fn a_stopped_text_job_is_asked_for_again() {
    let mut reader = reader_on(10);
    settle_to_text(&mut reader);
    reader.select_all();
    let read = settle_to_text(&mut reader).expect("what shows is read");
    let page = read.position;
    reader.finish(read, Outcome::Cancelled);
    let again = settle_to_text(&mut reader).expect("it's still wanted");
    assert_eq!(again.position, page);
    assert!(matches!(again.kind, JobKind::Text { check: false }));
    reader.finish(
        again,
        Outcome::Text {
            text: Arc::new(recto_index::select::PageText::default()),
            checked: false,
        },
    );
    let check = settle_to_text(&mut reader).expect("not tried, so asked for");
    assert!(matches!(check.kind, JobKind::Text { check: true }));
}

/// Failure: a tile finishes after the reader has left it, or for an old zoom or paper or a warm tab, and it's held in the footprint with nothing showing it. Only what's on a layer may count, or memory has no bound the budgets can rely on.
#[test]
fn a_tile_nothing_is_showing_when_it_arrives_is_discardable() {
    let mut reader = reader_on(100);
    let draw = loop {
        let job = reader.next_job().expect("a job");
        match job.kind {
            JobKind::Size => reader.finish(job, Outcome::Size(letter())),
            JobKind::Text { .. } => panic!("no text was asked for"),
            JobKind::Links => reader.finish(job, Outcome::Links(None)),
            JobKind::Traits => reader.finish(job, Outcome::Heavy(false)),
            JobKind::Draw { .. } => break job,
        }
    };
    reader.set_viewport(view(40_000.0));
    reader.finish(
        draw,
        Outcome::Drawn {
            kind: TileKind::Colour,
            low_res: None,
        },
    );
    let entry = reader.entries.values().next().expect("kept");
    assert!(!entry.attached && entry.discardable, "{entry:?}");
}

/// Failure: going to another tab and back sends the reader to the top of their page, so on a tall page, or any page read past its top, the place is lost. On the page it was left on, it comes back to the same place, and anywhere else it starts at the top.
#[test]
fn coming_back_to_a_tab_keeps_the_place_in_the_page() {
    let mut reader = reader_on(100);
    settle(&mut reader);
    let tops = |reader: &Reader, page| reader.doc().unwrap().layout.top(page);
    let inside = tops(&reader, 40) + 300.0;
    reader.set_viewport(view(inside));
    let page = reader.take_moved_page().expect("the reader moved");
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
    reader.take_ops();
    reader.activate(1, 100, page, map(), &[2], None, Spread::Single);
    let scrolled: Vec<f64> = reader
        .take_ops()
        .into_iter()
        .filter_map(|op| if let Op::ScrollTo { y, .. } = op { Some(y) } else { None })
        .collect();
    assert!((scrolled.last().unwrap() - inside).abs() < 1.0, "{scrolled:?} against {inside}");

    reader.activate(2, 10, 0, other, &[1], None, Spread::Single);
    reader.take_ops();
    reader.activate(1, 100, page + 3, map(), &[2], None, Spread::Single);
    let scrolled: Vec<f64> = reader
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

/// Failure: closing a warm tab leaves its layout, tiles, worker and open file until the next switch of tab, which may never come.
#[test]
fn a_warm_tab_that_goes_holds_nothing_at_once() {
    let mut reader = reader_on(10);
    settle(&mut reader);
    reader.activate(
        2,
        10,
        0,
        PageMap {
            files: vec![PathBuf::from("/d/b.pdf")],
            slots: None,
            own: None,
            turns: None,
            expected: Vec::new(),
            follows: None,
        },
        &[1],
        None,
        Spread::Single,
    );
    assert!(reader.entries.values().any(|entry| entry.tab == 1), "still warm");
    assert_eq!(reader.wanted_files().len(), 2);
    reader.set_warm(&[]);
    assert!(reader.entries.values().all(|entry| entry.tab != 1), "closed");
    assert!(!reader.docs.contains_key(&1));
    assert_eq!(*reader.wanted_files(), vec![PathBuf::from("/d/b.pdf")], "and its worker goes");
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
    let inside = reader.doc().unwrap().layout.top(40) + 300.0;
    reader.set_viewport(Viewport {
        magnification: 2.0,
        ..view(inside)
    });
    reader.take_moved_page();
    settle(&mut reader);
    // As the old window got to, with its own count of scrolls.
    reader.scrolled_to = 0;
    reader.window_closed();
    let new_window = Viewport { scrolled_to: 0, ..view(0.0) };
    reader.set_viewport(new_window);
    assert_eq!(reader.take_moved_page(), None, "the new window isn't the reader moving");
    reader.window_opened();
    let ops = reader.set_viewport(new_window);
    assert_eq!(reader.take_moved_page(), None, "nor is it before it has got there");
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
    assert_eq!(reader.take_moved_page(), None, "arriving isn't a move");
    assert_eq!(reader.doc().unwrap().magnification, 2.0);
}

/// Failure (decision 25): a tab woken from its sleep file comes up with bare pages until they're drawn, or a picture's used for the wrong page or paper, or lands on the wrong tab. It's on the layers in the first turn with nothing drawn, and it's kept as a preview, so sharp tiles replace it as usual.
#[test]
fn the_tabs_sleep_view_is_its_first_frame() {
    let mut reader = Reader::new();
    reader.set_viewport(view(0.0));
    let surface = PixelSurface::create(153, 198).unwrap();
    let shot = |page: u32, surface: PixelSurface| SleepShot {
        page,
        scale: 0.25,
        paper: PaperKind::White,
        surface,
    };
    let saved = SavedPlace {
        page: 0,
        anchor: (0, 0.0),
        x: 0.0,
        y: 0.0,
        document_height: 0.0,
        magnification: 1.0,
        page_size: Some(letter()),
    };
    // For another tab: not used.
    reader.give_sleep(
        9,
        SleepView {
            version: None,
            usual: Some(letter()),
            sizes: Vec::new(),
            pictures: vec![shot(0, PixelSurface::create(153, 198).unwrap())],
        },
    );
    reader.note_wake(1, Instant::now());
    reader.give_sleep(
        1,
        SleepView {
            version: None,
            usual: Some(letter()),
            sizes: vec![(3, PageBox { width: 300.0, height: 400.0 })],
            pictures: vec![shot(0, surface), shot(500, PixelSurface::create(153, 198).unwrap())],
        },
    );
    reader.activate(1, 5, 0, map(), &[], Some(saved), Spread::Single);
    let doc = reader.doc().unwrap();
    assert!(doc.layout.is_known(0));
    assert!(doc.layout.is_known(3), "the page table's taken up");
    assert_eq!(doc.layout.size(3), PageBox { width: 300.0, height: 400.0 });
    assert_eq!(reader.entries.len(), 1, "the one for a page that's there");
    let scrolled = reader.jump.unwrap_or(0);
    let ops = reader.set_viewport(Viewport {
        scrolled_to: scrolled,
        ..view(0.0)
    });
    let attached = reader.attached.get(&0).is_some_and(|ids| !ids.is_empty());
    assert!(attached, "it's up with nothing drawn: {ops:?}");
    assert!(ops.iter().any(|op| matches!(op, Op::Attach { below: true, .. })));
    reader.take_ops();
    reader.unapplied = true;
    reader.applied();
    let wake = reader.last_wake().unwrap();
    assert!(wake.from_sleep);
    assert!(wake.first_frame_us.is_some(), "{wake:?}");
    assert_eq!(wake.sharp_us, None, "not sharp yet");
    assert_eq!(wake.blank_frames, 0);
    // With no sleep file and the window up, a bare page is counted, which is what the budget's about.
    let mut bare = Reader::new();
    bare.window_up();
    bare.set_viewport(view(0.0));
    bare.note_wake(1, Instant::now());
    bare.activate(1, 5, 0, map(), &[], Some(saved), Spread::Single);
    bare.take_ops();
    let scrolled = bare.jump.unwrap_or(0);
    bare.set_viewport(Viewport {
        scrolled_to: scrolled,
        ..view(0.0)
    });
    bare.take_ops();
    bare.unapplied = true;
    bare.applied();
    assert_eq!(bare.last_wake().unwrap().blank_frames, 1);
    // A worker's asked for what's sharp, as if there'd been none of this, and no preview as it's got one.
    let jobs = settle(&mut reader);
    assert!(
        !jobs
            .iter()
            .any(|job| matches!(job.kind, JobKind::Draw { what: DrawKind::Preview, .. }) && job.position == 0)
    );
}

/// Failure: a sleep file has no picture of a heavy page, which has no preview, so it wakes bare; or what's put together of its tiles is the wrong size or in the wrong place; or a page's picture comes from a preview the system's taken. It's a real picture of the page at a smaller scale, and pixels of a tile land where they should.
#[test]
fn a_picture_for_sleep_comes_from_a_preview_or_the_tiles() {
    let mut reader = reader_on(5);
    assert!(
        reader.sleep_material(1, None).unwrap().pictures.is_empty(),
        "no worker's said what version this is, so no picture can say"
    );
    reader.note_version(Path::new("/d/a.pdf"), 1);
    settle(&mut reader);
    let material = reader.sleep_material(1, None).expect("a tab it holds");
    assert!(
        reader
            .sleep_material(
                1,
                Some(FileStamp {
                    size: 1,
                    mtime_ns: 1,
                    device: 0,
                    inode: 1
                })
            )
            .is_none(),
        "what's here is of another version of the file than that"
    );
    assert_eq!(material.usual, Some(letter()));
    assert!(!material.pictures.is_empty());
    let picture = &material.pictures[0];
    assert_eq!(picture.page, 0);
    assert_eq!(picture.bgra.len(), picture.width as usize * picture.height as usize * 4);
    assert!(reader.sleep_material(99, None).is_none());
    // Pixels from a tile, shrunk by two: a block of 2 by 2 comes to one, and their average, and it lands at the tile's place.
    let mut tile = vec![0u8; 4 * 4 * 4];
    for pixel in tile.as_chunks_mut::<4>().0 {
        *pixel = [100, 100, 100, 255];
    }
    tile[0..4].copy_from_slice(&[200, 200, 200, 255]);
    let mut picture = vec![9u8; 4 * 4 * 4];
    shrink_into(&tile, 16, 4, 4, 2, (1, 1), &mut picture, 4, 4);
    let at = |x: usize, y: usize| picture[(y * 4 + x) * 4];
    assert_eq!(at(0, 0), 9, "outside it, left alone");
    assert_eq!(at(1, 1), 125, "(200 + 3 x 100) / 4");
    assert_eq!(at(2, 2), 100);
    assert_eq!(at(3, 3), 9);
    // Off the edge of the picture: nothing panics.
    shrink_into(&tile, 16, 4, 4, 2, (3, 3), &mut picture, 4, 4);
    shrink_into(&tile[..8], 16, 4, 4, 2, (0, 0), &mut picture, 4, 4);
}

/// Failure (review O5): a protected file's renderer finds it needs a password the keyring has none for, as when the file was moved to another volume or replaced while it was open, and the tab only says "needs a password", with no way to type one: the app has to hear of the file, once, to ask with the sheet. Another failure isn't that, and a password given clears it.
#[test]
fn a_file_the_renderer_has_no_password_for_is_told_to_the_app() {
    let mut reader = reader_on(3);
    let job = reader.next_job().expect("a job");
    reader.finish(
        job,
        Outcome::DocumentFailed {
            message: "This PDF needs a password.".into(),
            needs_key: true,
        },
    );
    assert_eq!(reader.take_keyless(), [PathBuf::from("/d/a.pdf")]);
    assert!(reader.take_keyless().is_empty(), "told once");
    let mut other = reader_on(3);
    let job = other.next_job().expect("a job");
    other.finish(
        job,
        Outcome::DocumentFailed {
            message: "Not a PDF.".into(),
            needs_key: false,
        },
    );
    assert!(other.take_keyless().is_empty(), "only for want of a password");
    let job = reader.next_job();
    if let Some(job) = job {
        reader.finish(
            job,
            Outcome::DocumentFailed {
                message: "This PDF needs a password.".into(),
                needs_key: true,
            },
        );
    }
    reader.key_given(Path::new("/d/a.pdf"));
    assert!(reader.take_keyless().is_empty(), "a password given takes it off the list");
}

/// Failure (review): a tab's file turns out to be another, by a change outside or a save, and its previews stay, so its next sleep file has the old file's pictures under the new file's name, for good, and with a redaction that picture's what was removed. Another version of the file is a change of what's on the pages, and everything of it goes, on screen and off.
#[test]
fn another_version_of_the_file_takes_every_picture_of_the_old_one() {
    let version = |size: u64| {
        Some(FileStamp {
            size,
            mtime_ns: 1,
            device: 0,
            inode: 7,
        })
    };
    let on = |own: Option<FileStamp>| PageMap { own, ..map() };
    let mut reader = Reader::new();
    reader.set_viewport(view(0.0));
    reader.activate(1, 5, 0, on(version(1)), &[], None, Spread::Single);
    reader.note_version(Path::new("/d/a.pdf"), 1);
    settle(&mut reader);
    assert!(reader.entries.values().any(|entry| entry.preview), "there's a preview");
    let material = reader.sleep_material(1, version(1)).unwrap();
    assert!(!material.pictures.is_empty());
    // Told it's another file: nothing of the old one's left, and the old one's not offered to the sleep file under the new name.
    assert!(reader.sleep_material(1, version(2)).is_none());
    reader.activate(1, 5, 0, on(version(2)), &[], None, Spread::Single);
    assert!(reader.entries.is_empty(), "every picture and tile of it goes");
    assert!(reader.sleep_material(1, version(2)).unwrap().pictures.is_empty());
    assert!(reader.take_ops().contains(&Op::Clear), "and off the screen");
    assert_eq!(
        reader.take_reopen_files(),
        [PathBuf::from("/d/a.pdf")],
        "and a worker that has the old one open goes, or it goes on drawing that"
    );
}

/// Failure (review, the red frames): a tab's file is replaced while its tiles and previews are held, and they go on being shown, or written in its sleep file, as nothing that drew them says which version they're of. Each says, and as soon as a worker says the file's another version, whatever's of the old one goes, from the screen too, and nothing of it can be put back on a layer or into a sleep file.
#[test]
fn nothing_drawn_of_an_old_version_is_shown_or_kept() {
    let file = Path::new("/d/a.pdf");
    let mut reader = reader_on(5);
    reader.note_version(file, 1);
    settle(&mut reader);
    let drawn: Vec<u64> = reader.entries.keys().copied().collect();
    assert!(!drawn.is_empty() && reader.entries.values().all(|entry| entry.version == Some(1)));
    // Off screen and back: it would come straight back from what's kept.
    reader.set_viewport(view(20_000.0));
    settle(&mut reader);
    reader.note_version(file, 2);
    assert!(drawn.iter().all(|id| !reader.entries.contains_key(id)), "everything of version 1 is gone");
    reader.set_viewport(view(0.0));
    let ops = reader.take_ops();
    assert!(
        !ops.iter().any(|op| matches!(op, Op::Attach { id, .. } if drawn.contains(id))),
        "nothing of it goes back on a layer"
    );
    assert!(
        reader.sleep_material(1, None).unwrap().pictures.is_empty(),
        "and nothing's offered for a sleep file till it's drawn again"
    );
    settle(&mut reader);
    assert!(reader.entries.values().all(|entry| entry.version == Some(2)));
    assert!(!reader.sleep_material(1, None).unwrap().pictures.is_empty());
    // Something kept whose version is stale is refused at the moment it would go on a layer, however it got there.
    let stale = *reader.entries.keys().next().unwrap();
    if let Some(entry) = reader.entries.get_mut(&stale) {
        entry.version = Some(1);
        entry.attached = false;
    }
    assert!(!reader.attach(stale, false));
    assert!(!reader.entries.contains_key(&stale));
    // A picture from a sleep file, of a version a worker then says isn't this one, goes too.
    let mut woken = Reader::new();
    woken.set_viewport(view(0.0));
    let shot = SleepShot {
        page: 0,
        scale: 0.25,
        paper: PaperKind::White,
        surface: PixelSurface::create(153, 198).unwrap(),
    };
    woken.give_sleep(
        1,
        SleepView {
            version: Some(7),
            usual: Some(letter()),
            sizes: Vec::new(),
            pictures: vec![shot],
        },
    );
    woken.activate(1, 5, 0, map(), &[], None, Spread::Single);
    assert_eq!(woken.entries.len(), 1);
    woken.note_version(file, 8);
    assert!(woken.entries.is_empty());
}

/// Failure: memory pressure takes what's on screen, leaving a blank page, or leaves what could go. And at its worst, the previews go too.
#[test]
fn memory_pressure_leaves_only_whats_showing() {
    let mut reader = reader_on(30);
    settle(&mut reader);
    reader.set_viewport(view(5000.0));
    settle(&mut reader);
    let attached = reader.attached.values().map(Vec::len).sum::<usize>();
    assert!(reader.entries.len() > attached, "there's something off screen to go");
    reader.memory_pressure(false);
    let previews = reader.entries.values().filter(|entry| !entry.attached && entry.preview).count();
    assert!(
        reader.entries.values().all(|entry| entry.attached || entry.preview),
        "only what's showing, and previews"
    );
    assert!(previews > 0);
    reader.memory_pressure(true);
    assert_eq!(reader.entries.len(), attached);
    assert_eq!(reader.attached.values().map(Vec::len).sum::<usize>(), attached, "nothing's come off the screen");
}

/// Failure: a window can't go up until every tile's drawn, which on a heavy page is most of a second, or it goes up over a bare page.
#[test]
fn a_window_can_go_up_with_a_picture_on_every_page() {
    let mut reader = Reader::new();
    reader.set_viewport(view(0.0));
    let saved = SavedPlace {
        page: 0,
        anchor: (0, 0.0),
        x: 0.0,
        y: 0.0,
        document_height: 0.0,
        magnification: 1.0,
        page_size: Some(letter()),
    };
    reader.give_sleep(
        1,
        SleepView {
            version: None,
            usual: Some(letter()),
            sizes: Vec::new(),
            pictures: Vec::new(),
        },
    );
    reader.activate(1, 5, 0, map(), &[], Some(saved), Spread::Single);
    reader.take_ops();
    assert!(!reader.is_showable(), "still being sent to its place, with nothing on it");
    let scrolled = reader.jump.unwrap_or(0);
    reader.set_viewport(Viewport {
        scrolled_to: scrolled,
        ..view(0.0)
    });
    reader.take_ops();
    reader.unapplied = false;
    assert!(!reader.is_showable(), "a bare page isn't what goes up");
    let shot = SleepShot {
        page: 0,
        scale: 0.25,
        paper: PaperKind::White,
        surface: PixelSurface::create(153, 198).unwrap(),
    };
    reader.give_sleep(
        2,
        SleepView {
            version: None,
            usual: Some(letter()),
            sizes: Vec::new(),
            pictures: vec![shot],
        },
    );
    reader.activate(2, 1, 0, map(), &[], Some(saved), Spread::Single);
    reader.take_ops();
    let scrolled = reader.jump.unwrap_or(0);
    reader.set_viewport(Viewport {
        scrolled_to: scrolled,
        ..view(0.0)
    });
    reader.take_ops();
    reader.unapplied = false;
    assert!(reader.is_showable());
    assert!(!reader.is_on_place(), "and that's sooner than sharp");
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
    };
    reader.activate(1, 100, 40, map(), &[], Some(saved), Spread::Single);
    let ops = reader.take_ops();
    assert!(ops.iter().any(|op| matches!(op, Op::Magnify { value } if *value == 1.75)), "{ops:?}");
    assert!(ops.iter().any(|op| matches!(op, Op::ScrollAcross { x } if *x == 33.5)), "{ops:?}");
    let Some(Op::ScrollTo { y: went, seq }) = ops.iter().rev().find(|op| matches!(op, Op::ScrollTo { .. })) else {
        panic!("{ops:?}")
    };
    assert_eq!(*went, y, "exactly");
    // Not there yet: where the view says it is, isn't the place.
    reader.scrolled_to = 0;
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
    assert_eq!(reader.take_moved_page(), None, "arriving isn't a move");
    // Left and back in the same session.
    reader.activate(2, 10, 0, map(), &[1], None, Spread::Single);
    reader.activate(1, 100, 40, map(), &[2], None, Spread::Single);
    let ops = reader.take_ops();
    let Some(Op::ScrollTo { y: went, .. }) = ops.iter().rev().find(|op| matches!(op, Op::ScrollTo { .. })) else {
        panic!("{ops:?}")
    };
    assert_eq!(*went, y);
    // Another page since: its top, and no zoom it shouldn't have kept is the place's business.
    let mut fresh = Reader::new();
    fresh.window_opened();
    fresh.set_viewport(view(0.0));
    fresh.activate(1, 100, 7, map(), &[], Some(saved), Spread::Single);
    let ops = fresh.take_ops();
    let Some(Op::ScrollTo { y: went, .. }) = ops.iter().rev().find(|op| matches!(op, Op::ScrollTo { .. })) else {
        panic!("{ops:?}")
    };
    assert_eq!(*went, fresh.doc().unwrap().layout.top(7));
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
    for op in hostile.take_ops() {
        match op {
            Op::ScrollTo { y, .. } => assert!(y.is_finite(), "{y}"),
            Op::ScrollAcross { x } => assert!(x.is_finite(), "{x}"),
            Op::Magnify { value } => assert!(value.is_finite() && (MIN_ZOOM..=MAX_ZOOM).contains(&value), "{value}"),
            _ => {}
        }
    }
}

/// Failure: closing the window leaves ops queued with nobody to take them. Each `Attach` holds a surface's memory until the window's back, and `qa wait-settled` waits for ever. They're gone, and the surfaces with them, the moment it's closed.
#[test]
fn a_closed_window_keeps_no_ops_or_surfaces_for_them() {
    let mut reader = reader_on(10);
    let draw = a_draw_job(&mut reader, DrawKind::Preview);
    let JobKind::Draw { surface, .. } = &draw.kind else { panic!() };
    let held = surface.clone();
    reader.finish(
        draw,
        Outcome::Drawn {
            kind: TileKind::Colour,
            low_res: None,
        },
    );
    // Attached in Rust, and waiting for a host that hasn't come for it.
    assert!(
        reader.ops.iter().any(|op| matches!(op, Op::Attach { surface, .. } if *surface != 0)),
        "{:?}",
        reader.ops
    );
    let before = held.reference_count();
    reader.window_closed();
    assert!(reader.ops.is_empty(), "{:?}", reader.ops);
    assert!(
        held.reference_count() < before - 1,
        "the entry's and the waiting op's both gone: {} from {before}",
        held.reference_count()
    );
    assert!(reader.is_settled(), "{}", reader.report().0);
}

/// Failure: the host's size change moves the reader away from what they were reading.
#[test]
fn a_resize_keeps_the_reader_on_their_page() {
    let mut reader = reader_on(100);
    settle(&mut reader);
    let y = reader.doc().unwrap().layout.top(40);
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
    let top = reader.doc().unwrap().layout.top(40);
    assert!((*new_y - top).abs() < 1.0, "{new_y} against {top}");
}

/// Failure: pointing at a page never asks for its links, so the hand and the hint never come; or a link's answer about another tab lands on this one; or a click on a link isn't seen as one.
#[test]
fn a_page_pointed_at_has_its_links_read_and_a_click_found() {
    use recto_core::nav::{Link, LinkTarget, PageLinks};
    let mut reader = reader_on(10);
    settle(&mut reader);
    let (x, y) = reader.document_point(0, 50.0, 20.0).unwrap();
    assert_eq!(reader.pointer_link(x, y), None, "not read yet");
    let job = reader.next_job().expect("its links are asked for first");
    assert!(matches!(job.kind, JobKind::Links) && job.position == 0, "{job:?}");
    let links = PageLinks {
        links: vec![Link {
            rect: [10.0, 10.0, 100.0, 30.0],
            target: LinkTarget::Page { page: 4, within: None },
        }],
        ..PageLinks::default()
    };
    reader.finish(job, Outcome::Links(Some(Arc::new(links))));
    let hit = reader.pointer_link(x, y).expect("now it's known");
    assert_eq!(hit.target, LinkTarget::Page { page: 4, within: None });
    assert_eq!(reader.position_in_same_file(0, 4), Some(4));
    reader.selection_press(x, y, 1, false);
    reader.selection_release();
    assert_eq!(reader.take_clicked_link().map(|hit| hit.page), Some(0));
    // Another tab: what's known goes with the one that was showing.
    reader.activate(2, 10, 0, map(), &[], None, Spread::Single);
    assert_eq!(reader.link_pages_held(), 0);
}

fn kind_of(op: &Op) -> &'static str {
    match op {
        Op::Clear => "Clear",
        Op::Document { .. } => "Document",
        Op::Magnify { .. } => "Magnify",
        Op::ScrollTo { .. } => "ScrollTo",
        Op::ScrollAcross { .. } => "ScrollAcross",
        Op::ShowPage { .. } => "ShowPage",
        Op::HidePage { .. } => "HidePage",
        Op::Attach { .. } => "Attach",
        Op::Detach { .. } => "Detach",
        Op::Highlight { .. } => "Highlight",
    }
}

/// [`settle`], keeping every op the host is given, in order.
fn settle_recording(reader: &mut Reader, seen: &mut Vec<Op>) {
    for _ in 0..10_000 {
        seen.extend(reader.take_ops());
        reader.applied();
        let Some(job) = reader.next_job() else { break };
        let outcome = match &job.kind {
            JobKind::Size => Outcome::Size(letter()),
            JobKind::Text { .. } => panic!("no text was asked for"),
            JobKind::Links => Outcome::Links(None),
            JobKind::Traits => Outcome::Heavy(false),
            JobKind::Draw { .. } => Outcome::Drawn {
                kind: TileKind::Colour,
                low_res: None,
            },
        };
        reader.finish(job, outcome);
        seen.extend(reader.drain(MAX_ATTACH_PER_TURN));
        reader.applied();
    }
    for _ in 0..100 {
        if !reader.has_ops() {
            break;
        }
        seen.extend(reader.drain(MAX_ATTACH_PER_TURN));
        reader.applied();
    }
}

/// Failure: the host applies a batch in the order it's given, so a batch out of order shows wrongly: a `Clear` after a tile's `Attach` leaves the page bare, a `Magnify` after the `ScrollTo` scrolls at the old zoom, a `ShowPage` before the `Document` places a page in a document of the old size, and a `Detach` of a tile that isn't on a layer, or an `Attach` of one that is, leaves a layer the host can't take off. Checked on a first activation, a tab switch and back, a scroll down and up, and a window closed and opened again.
#[test]
fn the_host_gets_each_batch_in_the_order_it_applies_it() {
    const ORDER: [&str; 7] = ["Clear", "Magnify", "Document", "ScrollTo", "ScrollAcross", "ShowPage", "Attach"];
    let in_order = |what: &str, ops: &[Op], needed: &[&str]| {
        let kinds: Vec<&str> = ops.iter().map(kind_of).collect();
        for kind in needed {
            assert!(kinds.contains(kind), "{what}: no {kind} in {kinds:?}");
        }
        let firsts: Vec<usize> = ORDER.iter().filter_map(|kind| kinds.iter().position(|seen| seen == kind)).collect();
        assert!(firsts.windows(2).all(|pair| pair[0] < pair[1]), "{what}: {kinds:?}");
    };
    // The host's layers, as the ops say: every `Detach` is of a tile on one, and every `Attach` of one that isn't. A page's `HidePage` takes its tiles with it.
    let layers_hold = |what: &str, ops: &[Op]| {
        let mut on: HashMap<u64, u32> = HashMap::new();
        for op in ops {
            match op {
                Op::Clear => on.clear(),
                // A page's layer goes with its tiles.
                Op::HidePage { page } => on.retain(|_, on_page| on_page != page),
                Op::Attach { id, page, .. } => assert!(on.insert(*id, *page).is_none(), "{what}: {id} attached twice"),
                Op::Detach { id } => assert!(on.remove(id).is_some(), "{what}: {id} detached while not attached"),
                _ => {}
            }
        }
    };
    let mut reader = Reader::new();
    let mut all = reader.set_viewport(view(0.0));
    reader.activate(1, 12, 0, map(), &[], None, Spread::Single);
    let first = reader.take_ops();
    in_order("a first activation", &first, &["Clear", "Magnify", "Document", "ScrollTo", "ScrollAcross"]);
    all.extend(first);
    all.extend(reader.set_viewport(view(0.0)));
    settle_recording(&mut reader, &mut all);
    for step in (1..=10).chain((0..10).rev()) {
        all.extend(reader.set_viewport(view(f64::from(step) * 700.0)));
        settle_recording(&mut reader, &mut all);
    }
    reader.activate(2, 12, 0, map(), &[1], None, Spread::Single);
    let switch = reader.take_ops();
    in_order("a switch to a new tab", &switch, &["Clear", "Magnify", "Document", "ScrollTo", "ScrollAcross"]);
    all.extend(switch);
    all.extend(reader.set_viewport(view(0.0)));
    settle_recording(&mut reader, &mut all);
    reader.activate(1, 12, 0, map(), &[2], None, Spread::Single);
    let back = reader.take_ops();
    in_order("a switch back to a tab that's drawn", &back, &ORDER);
    all.extend(back);
    all.extend(reader.set_viewport(view(0.0)));
    settle_recording(&mut reader, &mut all);
    layers_hold("before the window closed", &all);
    reader.window_closed();
    reader.window_opened();
    let reopened = reader.take_ops();
    in_order(
        "a window opened again",
        &reopened,
        &["Magnify", "Document", "ScrollTo", "ScrollAcross", "ShowPage"],
    );
    let mut after = reopened;
    after.extend(reader.set_viewport(view(0.0)));
    settle_recording(&mut reader, &mut after);
    for step in 1..=6 {
        after.extend(reader.set_viewport(view(f64::from(step) * 700.0)));
        settle_recording(&mut reader, &mut after);
    }
    assert!(after.iter().any(|op| matches!(op, Op::Detach { .. })), "the scroll took tiles off");
    layers_hold("after the window opened again", &after);
}

/// What the page view allocates while the reader scrolls, counted on the thread that scrolls.
#[allow(unsafe_code)]
mod cost {
    use super::*;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    struct Counting;

    thread_local! {
        static ON: Cell<bool> = const { Cell::new(false) };
        static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
    }

    fn count() {
        // A thread on its way out has no counter any more.
        let _ = ON.try_with(|on| {
            if on.get() {
                let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
            }
        });
    }

    // SAFETY: forwards to the system allocator, counting each allocation and growth on the thread that makes it while counting is on.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            count();
            // SAFETY: the caller's layout, passed on.
            unsafe { System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            count();
            // SAFETY: the caller's layout, passed on.
            unsafe { System.alloc_zeroed(layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            count();
            // SAFETY: the caller's block and sizes, passed on.
            unsafe { System.realloc(ptr, layout, new_size) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: given back as it was given.
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    #[global_allocator]
    static COUNTING: Counting = Counting;

    /// Allocations made on this thread by `work`.
    fn allocations_of(work: impl FnOnce()) -> u64 {
        let before = ALLOCATIONS.with(Cell::get);
        ON.with(|on| on.set(true));
        work();
        ON.with(|on| on.set(false));
        ALLOCATIONS.with(Cell::get) - before
    }

    /// Allocations in 50 steps of scrolling over pages already drawn and kept, each step a viewport, a drain and the host's word that it applied them, as the UI thread makes on every scroll, as counted on `main` before `Reader` was cut into components (2026-11-13). That's 22.5 a step, for a step that draws nothing new: a finding against rule 1, pinned as it is here and not fixed in R3.3.
    const SCROLL_ALLOCATIONS_ON_MAIN: u64 = 1125;

    /// Failure: scrolling over pages that are drawn and kept allocates more than it did, so something now allocates per frame or per scroll: a component that copies what it used to borrow, a list built where one was walked. Rule 1: nothing new per frame or per scroll.
    #[test]
    fn scrolling_over_drawn_pages_allocates_no_more_than_it_did() {
        let mut reader = reader_on(6);
        settle(&mut reader);
        for step in 1..=12 {
            reader.set_viewport(view(f64::from(step) * 600.0));
            settle(&mut reader);
        }
        reader.set_viewport(view(0.0));
        settle(&mut reader);
        let allocations = allocations_of(|| {
            for step in 0..50u32 {
                let y = f64::from(step % 25) * 280.0;
                drop(reader.set_viewport(view(y)));
                drop(reader.drain(MAX_ATTACH_PER_TURN));
                reader.applied();
            }
        });
        println!("50 scroll steps over drawn pages: {allocations} allocations");
        assert!(
            allocations <= SCROLL_ALLOCATIONS_ON_MAIN,
            "{allocations} allocations in 50 steps, against {SCROLL_ALLOCATIONS_ON_MAIN} on main"
        );
    }
}
