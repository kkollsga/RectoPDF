use super::*;

/// Failure: reading on through a long document pushes out what it just read, and a page's preview goes with its tiles, so coming back to it shows nothing at all. Measured at 40 pages a second on `rfc9110`: 84 blank steps on seen pages, all from this. Past the tile cap, tiles go and previews stay.
#[test]
fn previews_outlive_their_tiles() {
    let mut reader = reader_on(600);
    let step = reader.docs.shown().map(|doc| doc.layout.top(1)).unwrap();
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
    let doc = reader.docs.shown().unwrap();
    let key = super::super::cache::TileKey::preview(doc.tab, 0, reader.paper);
    assert!(reader.cache.cover.contains_key(&key), "the first page's preview is still here");
    let ops = reader.set_viewport(view(0.0));
    assert!(ops.iter().any(|op| matches!(op, Op::Attach { below: true, .. })), "and shows at once: {ops:?}");
    assert_eq!(reader.counts().blank_seen_steps, 0);
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
    let previews = |reader: &Reader| reader.cache.entries.values().filter(|entry| entry.preview && !entry.attached).count();
    assert!(previews(&reader) > MAX_KEPT_PREVIEWS / 3, "enough kept to see the share: {}", previews(&reader));
    reader.cache.set_share(3, &mut reader.host);
    assert!(previews(&reader) <= MAX_KEPT_PREVIEWS / 3, "{}", previews(&reader));
    reader.cache.set_share(0, &mut reader.host);
    assert_eq!(reader.cache.share, 1, "never none");
}

/// Failure: memory pressure takes what's on screen, leaving a blank page, or leaves what could go. And at its worst, the previews go too.
#[test]
fn memory_pressure_leaves_only_whats_showing() {
    let mut reader = reader_on(30);
    settle(&mut reader);
    reader.set_viewport(view(5000.0));
    settle(&mut reader);
    let attached = reader.cache.attached.values().map(Vec::len).sum::<usize>();
    assert!(reader.cache.entries.len() > attached, "there's something off screen to go");
    reader.memory_pressure(false);
    let previews = reader.cache.entries.values().filter(|entry| !entry.attached && entry.preview).count();
    assert!(
        reader.cache.entries.values().all(|entry| entry.attached || entry.preview),
        "only what's showing, and previews"
    );
    assert!(previews > 0);
    reader.memory_pressure(true);
    assert_eq!(reader.cache.entries.len(), attached);
    assert_eq!(
        reader.cache.attached.values().map(Vec::len).sum::<usize>(),
        attached,
        "nothing's come off the screen"
    );
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
    let entry = reader.cache.entries.values().next().expect("kept");
    assert!(!entry.attached && entry.discardable, "{entry:?}");
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
        .cache
        .attached
        .values()
        .flatten()
        .filter_map(|id| reader.cache.entries.get(id))
        .filter(|entry| entry.scale == scale)
        .count() as u64;
    assert_eq!(attached, exact, "only exact tiles left");
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

/// Failure: a cold tab keeps its tiles, layout or renderer's file. Budget: 0 tiles and 0 handles for a cold tab.
#[test]
fn a_tab_gone_cold_holds_nothing() {
    let mut reader = reader_on(10);
    settle(&mut reader);
    assert!(!reader.cache.entries.is_empty());
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
    assert!(reader.cache.entries.values().all(|entry| entry.tab != 1), "tab 1 isn't warm");
    assert!(!reader.docs.views.contains_key(&1));
    assert_eq!(*reader.work.wanted_files(), vec![PathBuf::from("/d/b.pdf")]);
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
    assert_eq!(reader.work.wanted_files().len(), 2, "a warm tab's file is kept");
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
    assert!(reader.cache.entries.values().any(|entry| entry.tab == 1), "still warm");
    assert_eq!(reader.work.wanted_files().len(), 2);
    reader.set_warm(&[]);
    assert!(reader.cache.entries.values().all(|entry| entry.tab != 1), "closed");
    assert!(!reader.docs.views.contains_key(&1));
    assert_eq!(*reader.work.wanted_files(), vec![PathBuf::from("/d/b.pdf")], "and its worker goes");
}

/// Failure: the window closed still holds tiles, so a closed window stays over budget.
#[test]
fn a_closed_window_holds_no_tiles_and_comes_back() {
    let mut reader = reader_on(10);
    settle(&mut reader);
    reader.window_closed();
    assert!(reader.cache.entries.is_empty());
    assert!(reader.next_job().is_none(), "nothing draws for a closed window");
    assert!(reader.work.wanted_files().is_empty(), "the renderer goes too");
    reader.window_opened();
    reader.set_viewport(view(0.0));
    settle(&mut reader);
    assert!(reader.is_settled());
    assert!(!reader.cache.entries.is_empty());
}
