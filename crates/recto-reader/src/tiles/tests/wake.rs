use super::*;
use crate::tiles::wake::{MAX_LEFT_PICTURE_BYTES, MAX_SLEEP_SHOTS, WakeStock, shrink_into};

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
    assert_eq!(reader.cache.entries.len(), 1, "the one for a page that's there");
    let scrolled = reader.place.jump.unwrap_or(0);
    let ops = reader.set_viewport(Viewport {
        scrolled_to: scrolled,
        ..view(0.0)
    });
    let attached = reader.cache.attached.get(&0).is_some_and(|ids| !ids.is_empty());
    assert!(attached, "it's up with nothing drawn: {ops:?}");
    assert!(ops.iter().any(|op| matches!(op, Op::Attach { below: true, .. })));
    reader.host.take_ops();
    reader.host.unapplied = true;
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
    bare.host.take_ops();
    let scrolled = bare.place.jump.unwrap_or(0);
    bare.set_viewport(Viewport {
        scrolled_to: scrolled,
        ..view(0.0)
    });
    bare.host.take_ops();
    bare.host.unapplied = true;
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
    reader.place.scrolled_to = 0;
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
            .cache
            .entries
            .values()
            .any(|entry| entry.tab == 1 && entry.discardable && entry.surface.is_some()),
        "what the tab left showed is discardable now"
    );
    // What the kernel does to discardable memory when it needs pages.
    assert!(reader.empty_discardable() > 0);
    assert!(
        reader
            .cache
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
    assert!(reader.docs[&1].drawing.left_pictures.is_none());
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
    assert!(reader.docs[&1].drawing.left_pictures.is_none());
    assert!(
        reader.sleep_material(1, None).unwrap().pictures.is_empty(),
        "nothing of version 1 is left to give"
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
    reader.host.take_ops();
    assert!(!reader.is_showable(), "still being sent to its place, with nothing on it");
    let scrolled = reader.place.jump.unwrap_or(0);
    reader.set_viewport(Viewport {
        scrolled_to: scrolled,
        ..view(0.0)
    });
    reader.host.take_ops();
    reader.host.unapplied = false;
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
    reader.host.take_ops();
    let scrolled = reader.place.jump.unwrap_or(0);
    reader.set_viewport(Viewport {
        scrolled_to: scrolled,
        ..view(0.0)
    });
    reader.host.take_ops();
    reader.host.unapplied = false;
    assert!(reader.is_showable());
    assert!(!reader.is_on_place(), "and that's sooner than sharp");
}

/// A tab woken with the window up, from a sleep file with a picture for each of `pages` (none if empty), up to the host's first apply of what it was sent. The reader and its report.
fn woken_with(pages: &[u32], unattached: bool) -> (Reader, WakeReport) {
    let mut reader = Reader::new();
    reader.window_up();
    reader.set_viewport(view(0.0));
    reader.note_wake(1, Instant::now());
    let saved = SavedPlace {
        page: 0,
        anchor: (0, 0.0),
        x: 0.0,
        y: 0.0,
        document_height: 0.0,
        magnification: 1.0,
        page_size: Some(letter()),
    };
    if !pages.is_empty() {
        reader.give_sleep(
            1,
            SleepView {
                version: None,
                usual: Some(letter()),
                sizes: Vec::new(),
                pictures: pages
                    .iter()
                    .map(|page| SleepShot {
                        page: *page,
                        scale: 0.25,
                        paper: PaperKind::White,
                        surface: PixelSurface::create(153, 198).unwrap(),
                    })
                    .collect(),
            },
        );
    }
    reader.activate(1, 5, 0, map(), &[], Some(saved), Spread::Single);
    reader.host.take_ops();
    let scrolled = reader.place.jump.unwrap_or(0);
    reader.set_viewport(Viewport {
        scrolled_to: scrolled,
        ..view(0.0)
    });
    reader.host.take_ops();
    if unattached {
        // As if the picture were here and hadn't gone out with the first batch.
        reader.cache.attached.clear();
    }
    reader.host.unapplied = true;
    reader.applied();
    let wake = reader.last_wake().unwrap();
    (reader, wake)
}

/// Failure (2026-11-13, a long session under load counted bare frames and nothing said whether a picture had been owed): with nothing to show, the wake says so, and the bare frame it counts has no picture behind it.
#[test]
fn a_wake_with_nothing_to_show_says_so_and_a_bare_frame_has_no_picture() {
    let (_, wake) = woken_with(&[], false);
    assert_eq!(wake.had, WakeStock::Nothing, "{wake:?}");
    assert_eq!((wake.sleep_pictures, wake.sleep_in_view), (0, 0));
    assert_eq!(wake.blank_frames, 1, "bare until the renderer answers: {wake:?}");
    assert_eq!(wake.bare_with_picture, 0, "no picture was there to be late: {wake:?}");
}

/// Failure: a wake that began with a picture of its whole view, and still counts a bare frame, is a bug whatever the machine's load; the report must tell it from a wake that had nothing.
#[test]
fn a_wake_with_a_picture_of_the_view_is_covered_with_no_bare_frame() {
    let (_, wake) = woken_with(&[0], false);
    assert_eq!(wake.had, WakeStock::SleepFile, "{wake:?}");
    assert_eq!((wake.sleep_pictures, wake.sleep_in_view), (1, 1));
    assert!(wake.first_frame_us.is_some(), "{wake:?}");
    assert_eq!((wake.blank_frames, wake.bare_with_picture), (0, 0));
}

/// Failure: a sleep file whose pictures are of pages other than the ones at the place the tab was left at, was counted as something to show. It gave a picture, none in view, and the wake had nothing.
#[test]
fn a_sleep_file_whose_pictures_are_not_at_the_place_is_nothing_to_show() {
    let (_, wake) = woken_with(&[3], false);
    assert_eq!(wake.had, WakeStock::Nothing, "{wake:?}");
    assert_eq!((wake.sleep_pictures, wake.sleep_in_view), (1, 0));
    assert_eq!((wake.blank_frames, wake.bare_with_picture), (1, 0));
}

/// Failure: a picture that covers the view, here and not on a layer when the host applies a batch, is the bug the report exists to name, and was counted like any bare frame, with nothing to tell it from a wake that had nothing.
#[test]
fn a_picture_that_is_here_and_not_on_a_layer_is_counted_apart() {
    let (_, wake) = woken_with(&[0], true);
    assert_eq!(wake.had, WakeStock::SleepFile, "{wake:?}");
    assert_eq!((wake.blank_frames, wake.bare_with_picture), (1, 1), "{wake:?}");
}

/// Failure: the report grew past what's held for it, one for each page view.
#[test]
fn a_wake_report_stays_small() {
    assert_eq!(std::mem::size_of::<WakeReport>(), 48);
}
