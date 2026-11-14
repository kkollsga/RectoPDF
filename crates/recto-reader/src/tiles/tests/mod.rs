use super::cache::{MAX_KEPT_BYTES, MAX_KEPT_PREVIEWS};
use super::*;
use super::{MAX_ISSUES_REMEMBERED, MAX_SEEN_REMEMBERED};
use crate::layout::PageBox;
use recto_index::select::Range;
use recto_platform::surface::PixelSurface;
use recto_render_service::{LowResWhy, TileKind};
use std::collections::HashSet;
use std::path::Path;

mod cache;
mod host;
mod jobs;
mod look_again;
mod place;
mod pointer;
mod sleep_draw;
mod wake;
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
        if !reader.host.has_ops() {
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
        f64::from(reader.docs.shown().unwrap().drawing.tile_scale)
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
        let doc = reader.docs.shown().unwrap();
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
            let doc = reader.docs.shown().unwrap();
            for page in reader.visible_pages(doc) {
                let shows = !matches!(reader.visible_cells(doc, page, doc.drawing.tile_scale), Ok(cells) if cells.is_empty());
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
    assert_eq!(reader.docs.expected_stamp(1, Path::new("/d/b.pdf")), stamp, "tab 1 added it");
    assert_eq!(reader.docs.expected_stamp(2, Path::new("/d/b.pdf")), None, "it's tab 2's own file");
    assert_eq!(reader.docs.expected_stamp(1, Path::new("/d/a.pdf")), None, "and tab 1's own isn't checked so");
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
        let page = reader.docs.shown().unwrap().layout.rect(on);
        let mut at = view(page.y + page.height * 0.4);
        at.x = 25.0;
        at.magnification = 1.28;
        reader.set_viewport(at);
        reader.place.take_moved_page();
        let selected = Range {
            start: recto_index::select::Place { page: 2, at: 5 },
            end: recto_index::select::Place { page: 4, at: 9 },
        };
        reader.restore_selection(selected);
        let (fit, height, width, zoom) = {
            let doc = reader.docs.shown().unwrap();
            (
                doc.layout.fit(),
                doc.layout.document_height(),
                doc.layout.document_width(),
                doc.place.magnification,
            )
        };
        let rects: Vec<crate::layout::PageRect> = (0..10).map(|page| reader.docs.shown().unwrap().layout.rect(page)).collect();
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
        let doc = reader.docs.shown().unwrap();
        let label = format!("{spread:?}, renderer first: {renderer_saw_it_first}, on page {on}");
        assert_eq!(doc.layout.fit(), fit, "{label}: the fit, and so the zoom shown");
        assert_eq!(doc.place.magnification, zoom, "{label}: the magnification");
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
        assert_eq!(reader.pointer.selection_range(), Some(selected), "{label}: what was selected");
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
    assert_eq!(
        reader.pointer.selection_range(),
        None,
        "a save from another version than the one shown carries nothing"
    );
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
    assert_eq!(reader.docs.shown().unwrap().layout.count(), 2);
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
    let material = reader.sleep_material(1, &on(version(1))).unwrap();
    assert!(!material.pictures.is_empty());
    // Told it's another file: nothing of the old one's left, and the old one's not offered to the sleep file under the new name.
    assert!(reader.sleep_material(1, &on(version(2))).is_none());
    reader.activate(1, 5, 0, on(version(2)), &[], None, Spread::Single);
    assert!(reader.cache.entries.is_empty(), "every picture and tile of it goes");
    assert!(reader.sleep_material(1, &on(version(2))).unwrap().pictures.is_empty());
    assert!(reader.host.take_ops().contains(&Op::Clear), "and off the screen");
    assert!(
        reader.work.take_renewals().contains(&(PathBuf::from("/d/a.pdf"), crate::tiles::Renew::Reopen)),
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
        reader.sleep_material(1, &map()).unwrap().pictures.is_empty(),
        "and nothing's offered for a sleep file till it's drawn again"
    );
    settle(&mut reader);
    assert!(reader.cache.entries.values().all(|entry| entry.version == Some(2)));
    assert!(!reader.sleep_material(1, &map()).unwrap().pictures.is_empty());
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
    woken.wake.give_sleep(
        1,
        SleepView {
            version: Some(7),
            handed_back: false,
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

    /// Allocations in 50 steps of scrolling over pages already drawn and kept, each step a viewport, a drain and the host's word that it applied them, as the UI thread makes on every scroll. On `main` before R3.3 (2026-11-13) it was 1,125, 22.5 a step: the lists each refresh made, twice a step, the path of a page's file copied for each tile put on a layer, a list made for each page coming on, and the list of what went off made again after each applied. Now it's 56: the lists are kept, and what's left is one a batch of ops handed to the host, the `Vec` that becomes the host's (42 of the steps hand one, 7 hand two, one hands none).
    const SCROLL_ALLOCATIONS: u64 = 56;

    /// Scrolls `reader` over its six pages and back, so each is drawn and kept.
    fn drawn_and_kept(reader: &mut Reader) {
        settle(reader);
        for step in 1..=12 {
            reader.set_viewport(view(f64::from(step) * 600.0));
            settle(reader);
        }
        reader.set_viewport(view(0.0));
        settle(reader);
    }

    /// Failure: scrolling over pages that are drawn and kept allocates more than one `Vec` for each batch of ops it hands the host, so something allocates per frame or per scroll: a list built where one was walked or kept, a path or a key copied where it was borrowed. Rule 1: nothing new per frame or per scroll.
    #[test]
    fn scrolling_over_drawn_pages_allocates_only_what_it_hands_the_host() {
        let mut reader = reader_on(6);
        drawn_and_kept(&mut reader);
        let mut batches = 0u64;
        let allocations = allocations_of(|| {
            for step in 0..50u32 {
                let y = f64::from(step % 25) * 280.0;
                let ops = reader.set_viewport(view(y));
                let more = reader.drain(MAX_ATTACH_PER_TURN);
                batches += u64::from(!ops.is_empty()) + u64::from(!more.is_empty());
                drop((ops, more));
                reader.applied();
            }
        });
        println!("50 scroll steps over drawn pages: {allocations} allocations, {batches} batches of ops handed to the host");
        assert!(allocations <= batches, "{allocations} allocations for {batches} batches handed to the host");
        assert!(
            allocations <= SCROLL_ALLOCATIONS,
            "{allocations} allocations in 50 steps, against {SCROLL_ALLOCATIONS}"
        );
        // What the lists kept for scroll steps hold now, at rest on a hot tab: their room, within the caps.
        let held = reader.scratch.held_bytes() + reader.cache.spare_bytes();
        println!("kept for scroll steps at rest: {held} bytes, at most {}", Scratch::most_held_bytes());
        assert!(reader.scratch.held_bytes() <= Scratch::most_held_bytes());
    }

    /// Failure: a scroll step that moves no tile on or off a layer, a reader's slow scroll within the tiles in view, allocates. With nothing to tell the host, there's nothing to make.
    #[test]
    fn a_step_that_tells_the_host_nothing_allocates_nothing() {
        let mut reader = reader_on(6);
        drawn_and_kept(&mut reader);
        reader.set_viewport(view(10.0));
        settle(&mut reader);
        let mut batches = 0;
        let allocations = allocations_of(|| {
            for step in 0..50u32 {
                let y = 10.0 + f64::from(step % 5);
                let ops = reader.set_viewport(view(y));
                let more = reader.drain(MAX_ATTACH_PER_TURN);
                batches += usize::from(!ops.is_empty()) + usize::from(!more.is_empty());
                drop((ops, more));
                reader.applied();
            }
        });
        assert_eq!(batches, 0, "a scroll of a few points moved tiles, so this doesn't test a quiet step");
        assert_eq!(allocations, 0, "50 steps that told the host nothing");
    }

    /// **How long a scroll step takes, measured** (R3.6): the UI thread's whole part of a step over pages already drawn and kept, a viewport, a drain and the
    /// host's word that it applied them, as [`scrolling_over_drawn_pages_allocates_only_what_it_hands_the_host`] makes them. Not a check: it asserts nothing
    /// about time, which is one machine's, and runs only when asked, on a release build, before and after a change to the scroll path:
    ///
    /// `tools/sysguard/heavy.sh cargo nextest run --release --locked -p recto-reader --run-ignored only --no-capture -E 'test(=tiles::tests::cost::scroll_step_time)'`
    ///
    /// This binary counts allocations, which costs a thread-local look on each: the same before and after, so two builds compare.
    #[test]
    #[ignore = "a measurement, run on a release build when asked"]
    fn scroll_step_time() {
        let mut reader = reader_on(6);
        drawn_and_kept(&mut reader);
        for round in 0..5 {
            let mut steps = Vec::with_capacity(5_000);
            for step in 0..5_000u32 {
                // Down 24 steps of 280 points and back up, a scroll both ways: `step % 25` alone fell from 6,720 to 0 every 25th step, a jump across
                // the whole document that made the 99th percentile a jump's cost, not a scroll step's.
                let at_step = step % 48;
                let y = f64::from(if at_step <= 24 { at_step } else { 48 - at_step }) * 280.0;
                let began = std::time::Instant::now();
                let ops = reader.set_viewport(view(y));
                let more = reader.drain(MAX_ATTACH_PER_TURN);
                reader.applied();
                steps.push(began.elapsed());
                drop((ops, more));
            }
            steps.sort_unstable();
            let at = |share: f64| steps[((steps.len() - 1) as f64 * share) as usize].as_secs_f64() * 1e6;
            println!(
                "scroll step, round {round}: p50 {:.2} µs, p90 {:.2} µs, p99 {:.2} µs",
                at(0.5),
                at(0.9),
                at(0.99)
            );
        }
    }

    /// Failure: what's kept for scroll steps holds the room of the largest step it ever saw, as a hostile file's thousands of tiny pages in view, for as long as the reader lives; or a reader with nothing to show still holds it.
    #[test]
    fn the_room_kept_for_scroll_steps_is_capped_and_goes_with_the_tab() {
        let mut scratch = Scratch::default();
        scratch.visible.extend(0..100_000);
        scratch.going.extend(0..100_000);
        scratch.keys.reserve(100_000);
        scratch.reset();
        assert!(scratch.visible.is_empty() && scratch.going.is_empty());
        assert!(scratch.held_bytes() <= Scratch::most_held_bytes(), "{} bytes kept", scratch.held_bytes());

        let mut host = super::super::host::HostQueue::new();
        host.handed_to_host.extend(0..100_000);
        host.handed_to_host.clear();
        host.ops.extend((0..10_000).map(|page| Op::HidePage { page }));
        host.ops.clear();
        host.trim();
        assert!(host.handed_to_host.capacity() <= super::super::host::KEEP_GONE);
        assert!(host.ops.capacity() <= super::super::host::KEEP_OPS);

        let mut reader = reader_on(6);
        drawn_and_kept(&mut reader);
        for _ in 0..100 {
            reader.cache.give_back_list(Vec::with_capacity(4));
        }
        reader.cache.give_back_list(Vec::with_capacity(100_000));
        assert!(reader.cache.spare.len() <= super::super::cache::SPARE_LISTS);
        assert!(reader.cache.spare.iter().all(|list| list.capacity() <= super::super::cache::SPARE_LIST_ROOM));
        assert!(reader.scratch.held_bytes() > 0, "the scroll kept no room, so this doesn't test its going");
        reader.deactivate();
        assert_eq!(
            reader.scratch.held_bytes() + reader.cache.spare_bytes(),
            0,
            "a reader with no tab holds room for scroll steps"
        );
        let mut closed = reader_on(6);
        drawn_and_kept(&mut closed);
        closed.window_closed();
        assert_eq!(
            closed.scratch.held_bytes() + closed.cache.spare_bytes() + closed.host.held_bytes(),
            0,
            "a closed window holds room for scroll steps"
        );
    }
}

/// Failure: the page view remembers every page it has drawn, so a file that claims millions of pages, scrolled through, holds memory for each one (rules 1 and 6).
#[test]
fn the_pages_remembered_as_seen_stay_bounded() {
    let mut reader = reader_on(10);
    let doc = reader.docs.views.get_mut(&1).unwrap();
    let cap = u32::try_from(MAX_SEEN_REMEMBERED).unwrap();
    for page in 0..=cap {
        doc.drawing.note_seen(page);
    }
    assert!(doc.drawing.seen.len() <= MAX_SEEN_REMEMBERED, "{} remembered", doc.drawing.seen.len());
    assert!(doc.drawing.seen.contains(&cap), "the last page drawn is remembered");
}

/// Failure: the page view remembers the problem of every page that failed, so a hostile file whose millions of pages all fail holds memory for each (rules 1 and 6).
#[test]
fn the_pages_remembered_as_failing_stay_bounded() {
    let mut reader = reader_on(8_000_000);
    let cap = u32::try_from(MAX_ISSUES_REMEMBERED).unwrap();
    for page in 0..=cap {
        reader.note_issue(1, 1_000 + page, "It failed.".to_string(), false);
    }
    let issues = &reader.docs.views[&1].drawing.issues;
    assert!(issues.len() <= MAX_ISSUES_REMEMBERED, "{} remembered", issues.len());
    assert!(issues.contains_key(&(1_000 + cap)), "the last failure is remembered");
}

/// The view of a window `points` tall at the least zoom, in document points.
fn least_zoom_view(points: f64) -> Viewport {
    Viewport {
        height: points / MIN_ZOOM,
        width: 1168.0 / MIN_ZOOM,
        magnification: MIN_ZOOM,
        ..view(0.0)
    }
}

/// Failure: a hostile file's pages are so small that more of them are in view at once than the page view caps what it remembers by, so the problems of pages near the view, which the cap keeps, pass it (rules 1 and 6), and with them everything sized by the pages in view (layers placed, tiles on them, a step's ops). Pages of a hundredth of a point side by side are 10 points a row, the gap between rows; a window of AppKit's largest, 10,000 points tall, at the least zoom shows 40,000 points of them. And the cap that holds this cuts nothing from the view of the tallest Mac display, 3,384 points (a 6K display turned on its side, at its most space), at the least zoom.
#[test]
fn pages_in_view_are_held_below_what_the_caps_on_them_keep() {
    let mut reader = reader_on(8_000_000);
    reader.activate(1, 8_000_000, 0, map(), &[], None, Spread::TwoUp);
    let tiny = PageBox { width: 0.01, height: 0.01 };
    reader.docs.views.get_mut(&1).unwrap().layout.set_page_size(0, tiny);
    reader.set_viewport(least_zoom_view(3_384.0));
    let tallest_display = reader.visible_pages(reader.docs.shown().unwrap()).count();
    assert!(
        tallest_display < MAX_PAGES_IN_VIEW,
        "{tallest_display} pages in view of the tallest display are cut"
    );
    assert!(
        tallest_display > 2_000,
        "{tallest_display} pages in view: the pages aren't as small as this test needs"
    );
    reader.set_viewport(least_zoom_view(10_000.0));
    let in_view = reader.visible_pages(reader.docs.shown().unwrap());
    assert!(in_view.clone().count() <= MAX_PAGES_IN_VIEW, "{} pages in view", in_view.clone().count());
    for page in in_view.clone() {
        reader.note_issue(1, page, "It failed.".to_string(), false);
    }
    for page in 0..u32::try_from(MAX_ISSUES_REMEMBERED).unwrap() {
        reader.note_issue(1, in_view.end() + 100 + page, "It failed.".to_string(), false);
    }
    let issues = &reader.docs.views[&1].drawing.issues;
    assert!(issues.len() <= MAX_ISSUES_REMEMBERED, "{} problems remembered", issues.len());
    assert!(
        in_view.clone().all(|page| issues.contains_key(&page)),
        "a page in view lost its problem, so it's asked for at once"
    );
}

/// Failure: a failed page whose problem was let go at the cap is asked for again on every look once it's back in view, so the engine never rests; or a page in view loses its problem at the cap and is asked for at once.
#[test]
fn a_failed_page_let_go_at_the_cap_is_asked_for_once_when_its_back() {
    let mut reader = reader_on(20);
    reader.note_issue(1, 0, "It failed.".to_string(), false);
    let mut asked = 0;
    let run = |reader: &mut Reader, asked: &mut u32| {
        for _ in 0..1_000 {
            reader.host.take_ops();
            reader.applied();
            let Some(job) = reader.next_job() else { break };
            let outcome = if job.position == 0 {
                *asked += 1;
                Outcome::PageFailed {
                    message: "It failed again.".to_string(),
                    may_pass: false,
                }
            } else {
                match &job.kind {
                    JobKind::Size => Outcome::Size(letter()),
                    JobKind::Traits => Outcome::Heavy(false),
                    JobKind::Links => Outcome::Links(None),
                    _ => Outcome::Drawn {
                        kind: TileKind::Colour,
                        low_res: None,
                    },
                }
            };
            reader.finish(job, outcome);
            reader.drain(MAX_ATTACH_PER_TURN);
            reader.applied();
        }
    };
    run(&mut reader, &mut asked);
    assert_eq!(asked, 0, "its problem is remembered");
    reader.set_viewport(view(15.0 * 1100.0));
    run(&mut reader, &mut asked);
    let in_view = *reader.visible_pages(reader.docs.shown().unwrap()).start();
    reader.note_issue(1, in_view, "It failed.".to_string(), false);
    // Enough other failures to pass the cap, none of them near the view: page 0 is let go.
    for page in 0..u32::try_from(MAX_ISSUES_REMEMBERED).unwrap() {
        reader.note_issue(1, 100 + page, "It failed.".to_string(), false);
    }
    assert!(!reader.docs.views[&1].drawing.issues.contains_key(&0), "let go at the cap");
    assert!(reader.docs.views[&1].drawing.issues.contains_key(&in_view), "what's in view keeps its own");
    reader.set_viewport(view(0.0));
    run(&mut reader, &mut asked);
    for _ in 0..20 {
        reader.set_viewport(view(0.0));
        run(&mut reader, &mut asked);
    }
    assert_eq!(asked, 1, "asked for once on its way back, and remembered again");
}

/// Failure (a whole-suite run, 2026-11-13, and the sleep branch's second review): a tab put to sleep and woken from its sleep file while its worker lived
/// never had the file's length said again, as a worker says it once, so the session never checked the tab's sleep file and wrote it none after. It was asked
/// for only when a view was let go of, which a sleep and a wake in one look never do. A tab shown asks for its own file's length again, its view kept or
/// not, once however often it's shown before the thread that draws comes round, and the ask is gone once taken, so what's held is bounded by the files.
#[test]
fn a_tab_shown_has_its_files_length_said_again_once() {
    let b = PageMap {
        files: vec![PathBuf::from("/d/b.pdf")],
        ..map()
    };
    let mut reader = Reader::new();
    reader.set_viewport(view(0.0));
    reader.activate(1, 5, 0, map(), &[], None, Spread::Single);
    reader.activate(2, 5, 0, b.clone(), &[1], None, Spread::Single);
    reader.activate(1, 5, 0, map(), &[2], None, Spread::Single);
    assert_eq!(
        reader.work.take_renewals(),
        [
            (PathBuf::from("/d/a.pdf"), crate::tiles::Renew::TellLength(1)),
            (PathBuf::from("/d/b.pdf"), crate::tiles::Renew::TellLength(2))
        ],
        "each file shown asked once, a's view kept throughout"
    );
    reader.set_warm(&[]);
    assert!(reader.work.take_renewals().is_empty(), "taken, it's gone, and letting a view go asks nothing");
}
