use super::cache::{MAX_KEPT_BYTES, MAX_KEPT_PREVIEWS};
use super::*;
use crate::layout::PageBox;
use recto_index::select::Range;
use recto_render_service::LowResWhy;
use std::path::Path;

mod cache;
mod host;
mod jobs;
mod place;
mod pointer;
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
        reader.host.take_ops();
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
                    !shows || reader.cache.attached.get(&page).is_some_and(|ids| !ids.is_empty()),
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

/// The view the host goes to on the reader's next turn: wherever the last scroll it was told of says, as it would have applied it.
fn follow_host(reader: &mut Reader, mut at: Viewport) -> Viewport {
    if let Some(Op::ScrollTo { y, seq }) = reader.host.take_ops().into_iter().rfind(|op| matches!(op, Op::ScrollTo { .. })) {
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

/// Answers what the reader asks as a file whose page 0 is `first` and the rest letter would, until it asks nothing more.
fn settle_sized(reader: &mut Reader, first: PageBox) {
    for _ in 0..10_000 {
        reader.host.take_ops();
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
        reader.place.scrolled_to = 0;
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
    assert!(reader.cache.entries.is_empty());
    assert_eq!(reader.doc().unwrap().layout.count(), 2);
    assert_eq!(edited.source(0), Some((PathBuf::from("/d/a.pdf"), 1)));
    assert_eq!(edited.source(9), None);
}

/// Finishes everything that isn't text, with a pretend renderer, and hands back the first text job.
fn settle_to_text(reader: &mut Reader) -> Option<Job> {
    loop {
        reader.host.take_ops();
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
    assert!(reader.cache.entries.values().any(|entry| entry.preview), "there's a preview");
    let material = reader.sleep_material(1, version(1)).unwrap();
    assert!(!material.pictures.is_empty());
    // Told it's another file: nothing of the old one's left, and the old one's not offered to the sleep file under the new name.
    assert!(reader.sleep_material(1, version(2)).is_none());
    reader.activate(1, 5, 0, on(version(2)), &[], None, Spread::Single);
    assert!(reader.cache.entries.is_empty(), "every picture and tile of it goes");
    assert!(reader.sleep_material(1, version(2)).unwrap().pictures.is_empty());
    assert!(reader.host.take_ops().contains(&Op::Clear), "and off the screen");
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
    let drawn: Vec<u64> = reader.cache.entries.keys().copied().collect();
    assert!(!drawn.is_empty() && reader.cache.entries.values().all(|entry| entry.version == Some(1)));
    // Off screen and back: it would come straight back from what's kept.
    reader.set_viewport(view(20_000.0));
    settle(&mut reader);
    reader.note_version(file, 2);
    assert!(drawn.iter().all(|id| !reader.cache.entries.contains_key(id)), "everything of version 1 is gone");
    reader.set_viewport(view(0.0));
    let ops = reader.host.take_ops();
    assert!(
        !ops.iter().any(|op| matches!(op, Op::Attach { id, .. } if drawn.contains(id))),
        "nothing of it goes back on a layer"
    );
    assert!(
        reader.sleep_material(1, None).unwrap().pictures.is_empty(),
        "and nothing's offered for a sleep file till it's drawn again"
    );
    settle(&mut reader);
    assert!(reader.cache.entries.values().all(|entry| entry.version == Some(2)));
    assert!(!reader.sleep_material(1, None).unwrap().pictures.is_empty());
    // Something kept whose version is stale is refused at the moment it would go on a layer, however it got there.
    let stale = *reader.cache.entries.keys().next().unwrap();
    if let Some(entry) = reader.cache.entries.get_mut(&stale) {
        entry.version = Some(1);
        entry.attached = false;
    }
    assert!(!reader.attach(stale, false));
    assert!(!reader.cache.entries.contains_key(&stale));
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
    assert_eq!(woken.cache.entries.len(), 1);
    woken.note_version(file, 8);
    assert!(woken.cache.entries.is_empty());
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
