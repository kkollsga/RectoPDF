use super::*;
use crate::tiles::sleep_draw::{ASKING_FOR, Ended, MAX_JOBS, Next, PageNow, SleepDraws, TabNow, next};
use crate::tiles::wake::MAX_LEFT_PICTURE_BYTES;
use crate::tiles::wake::{RawPicture, in_view_of};
use std::time::{Duration, Instant};

/// What a job was, for a test to look at after it's gone.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Did {
    tab: u32,
    page: u32,
    kind: &'static str,
    scale: f32,
}

/// Runs the job queue with a pretend renderer until there's nothing left to do, as [`settle`] does, saying what each job was. A page in `heavy` is heavy.
fn run_jobs(reader: &mut Reader, heavy: &[(u32, u32)]) -> Vec<Did> {
    let mut did = Vec::new();
    for _ in 0..10_000 {
        reader.host.take_ops();
        reader.applied();
        let Some(job) = reader.next_job() else { break };
        let (kind, scale) = match &job.kind {
            JobKind::Size => ("size", 0.0),
            JobKind::Traits => ("weight", 0.0),
            JobKind::Draw {
                what: DrawKind::Preview,
                scale,
                ..
            } => ("preview", *scale),
            JobKind::Draw { scale, .. } => ("tile", *scale),
            JobKind::Text { .. } => ("text", 0.0),
            JobKind::Links => ("links", 0.0),
        };
        did.push(Did {
            tab: job.tab,
            page: job.position,
            kind,
            scale,
        });
        let outcome = match &job.kind {
            JobKind::Size => Outcome::Size(letter()),
            JobKind::Traits => Outcome::Heavy(heavy.contains(&(job.tab, job.position))),
            JobKind::Text { .. } => Outcome::TextFailed,
            JobKind::Links => Outcome::Links(None),
            JobKind::Draw { .. } => Outcome::Drawn {
                kind: TileKind::Colour,
                low_res: None,
            },
        };
        reader.note_started(&job, None);
        reader.finish(job, outcome);
    }
    did
}

fn other_tab(tab: u32) -> PageMap {
    PageMap {
        files: vec![PathBuf::from(format!("/d/{tab}.pdf"))],
        ..map()
    }
}

/// Tab 1, drawn at its top, then moved far down and left for tab 2 before anything's drawn there: the case the owner asked about.
fn left_undrawn() -> Reader {
    let mut reader = reader_on(40);
    reader.note_version(Path::new("/d/a.pdf"), 1);
    settle(&mut reader);
    reader.set_viewport(view(20_000.0));
    reader.activate(2, 10, 0, other_tab(2), &[1], None, Spread::Single);
    reader.set_viewport(view(0.0));
    reader
}

fn draws(reader: &Reader) -> serde_json::Value {
    reader.report().0["sleep_draws"].clone()
}

/// Failure (owner, 2026-11-13: "I think it makes sense, and extra draw is not costly right?"): a tab left at a place it has no picture of sleeps with none, and wakes to bare paper. Its place's pages are drawn, each a preview at the preview's own scale and nothing bigger, only once the tab showing has had everything it wants (the person's view first), and what's drawn is what its sleep file gets. Then nothing more's asked.
#[test]
fn a_tab_left_at_an_undrawn_place_has_it_drawn_after_the_tab_showing() {
    let mut reader = left_undrawn();
    let owed = reader.sleep_material(1, &map()).unwrap().in_view;
    assert!(!owed.is_empty() && !owed.contains(&0), "{owed:?}");
    let did = run_jobs(&mut reader, &[]);
    let first_for_left = did.iter().position(|job| job.tab == 1).expect("something's drawn for the tab left");
    assert!(did[..first_for_left].iter().any(|job| job.tab == 2), "the tab showing went first: {did:?}");
    assert!(
        did[first_for_left..].iter().all(|job| job.tab == 1),
        "nothing for the tab showing was left over: {did:?}"
    );
    let for_left: Vec<&Did> = did.iter().filter(|job| job.tab == 1).collect();
    assert!(for_left.iter().all(|job| owed.contains(&job.page)), "only the pages owed: {for_left:?}");
    let previews: Vec<u32> = for_left.iter().filter(|job| job.kind == "preview").map(|job| job.page).collect();
    assert_eq!(previews, owed, "one preview for each page owed");
    assert!(for_left.iter().all(|job| matches!(job.kind, "size" | "weight" | "preview")), "{for_left:?}");
    // At the scale a sleep picture of the page is, from the tab showing's: it's what a preview of tab 2 was drawn at, the pages being the same size.
    let preview_scale = did.iter().find(|job| job.tab == 2 && job.kind == "preview").unwrap().scale;
    assert!(for_left.iter().filter(|job| job.kind == "preview").all(|job| job.scale == preview_scale));
    // What a tab off screen has drawn is discardable, and the system takes it back whenever it likes: its sleep file has its place all the same.
    assert!(reader.empty_discardable() > 0);
    let material = reader.sleep_material(1, &map()).unwrap();
    let pictured: Vec<u32> = material
        .pictures
        .iter()
        .map(|picture| picture.page)
        .filter(|page| owed.contains(page))
        .collect();
    assert_eq!(pictured, owed, "its sleep file has its place");
    let report = draws(&reader);
    assert_eq!(report["leaves_owing"], 1, "{report}");
    assert_eq!(report["ended"]["done"], 1, "{report}");
    assert_eq!(report["pictures"], owed.len(), "{report}");
    assert!(run_jobs(&mut reader, &[]).is_empty(), "and nothing more");
    assert!(!SleepDraws::under_way(&reader));
}

/// Failure: the common case pays for the rare one. A tab left where everything in view was drawn, or a leave of no tab at all, asks the renderer for nothing more.
#[test]
fn a_tab_left_where_it_was_drawn_asks_for_nothing() {
    let mut reader = reader_on(40);
    reader.note_version(Path::new("/d/a.pdf"), 1);
    settle(&mut reader);
    reader.activate(2, 10, 0, other_tab(2), &[1], None, Spread::Single);
    reader.set_viewport(view(0.0));
    let did = run_jobs(&mut reader, &[]);
    assert!(did.iter().all(|job| job.tab == 2), "{did:?}");
    let report = draws(&reader);
    assert_eq!(report["leaves_pictured"], 1, "{report}");
    assert_eq!(report["leaves_owing"], 0, "{report}");
    assert_eq!(report["jobs"], 0, "{report}");
}

/// Failure: a person coming back to the tab waits behind drawing for its sleep, or that goes on drawing for a tab that's gone cold, or one whose worker has gone (starting one for it). Each ends it at once: back on screen it's drawn as any tab showing is, gone cold nothing more is asked, and with no worker nothing is.
#[test]
fn coming_back_going_cold_or_no_worker_ends_it() {
    let mut reader = left_undrawn();
    reader.activate(1, 40, 0, map(), &[2], None, Spread::Single);
    reader.set_viewport(view(0.0));
    let did = run_jobs(&mut reader, &[]);
    assert!(did.iter().all(|job| job.tab == 1), "{did:?}");
    assert_eq!(draws(&reader)["jobs"], 0, "only the tab showing's own: {}", draws(&reader));
    assert_eq!(draws(&reader)["ended"]["hot"], 1, "{}", draws(&reader));

    let mut reader = left_undrawn();
    reader.set_warm(&[]);
    assert!(run_jobs(&mut reader, &[]).iter().all(|job| job.tab == 2));
    assert_eq!(draws(&reader)["ended"]["cold"], 1, "{}", draws(&reader));

    let mut reader = left_undrawn();
    let job = settle_tab(&mut reader, 2).expect("a job for the tab left");
    assert!(SleepDraws::under_way(&reader));
    // What the engine says of a job its renderer wouldn't start a worker for.
    reader.finish(job, Outcome::Unstarted { heavy: false });
    assert!(!SleepDraws::under_way(&reader));
    assert!(run_jobs(&mut reader, &[]).is_empty(), "nothing's asked once its worker's gone");
    assert_eq!(draws(&reader)["ended"]["no_worker"], 1);
}

/// Draws what the tab showing wants, and stops at the first job for another tab, which it leaves running.
fn settle_tab(reader: &mut Reader, tab: u32) -> Option<Job> {
    for _ in 0..10_000 {
        reader.host.take_ops();
        reader.applied();
        let job = reader.next_job()?;
        reader.note_started(&job, None);
        if job.tab != tab {
            return Some(job);
        }
        let outcome = match &job.kind {
            JobKind::Size => Outcome::Size(letter()),
            JobKind::Traits => Outcome::Heavy(false),
            JobKind::Text { .. } => Outcome::TextFailed,
            JobKind::Links => Outcome::Links(None),
            JobKind::Draw { .. } => Outcome::Drawn {
                kind: TileKind::Colour,
                low_res: None,
            },
        };
        reader.finish(job, outcome);
    }
    None
}

/// Failure: a draw for a tab off screen holds up the person's view. Running, it's wanted only as long as the tab showing wants nothing: a scroll to a place not drawn stops it.
#[test]
fn the_view_wanting_something_stops_a_draw_for_a_tab_off_screen() {
    let mut reader = left_undrawn();
    let job = settle_tab(&mut reader, 2).expect("a job for the tab left");
    assert_eq!(job.tab, 1);
    assert!(reader.running_is_wanted(), "wanted while the view wants nothing");
    reader.set_viewport(view(6_000.0));
    assert!(!reader.running_is_wanted(), "the view wants its tiles first");
}

/// Failure: thirty tabs left in turn have thirty pages' worth asked for at once, and hold it all. It's one tab at a time, the one left last, at most a sleep file's pictures of pages.
#[test]
fn one_tab_at_a_time_the_one_left_last() {
    let mut reader = left_undrawn();
    // Tab 2 moved and left for tab 3 before anything's drawn for either.
    reader.note_version(Path::new("/d/2.pdf"), 1);
    reader.set_viewport(view(5_000.0));
    reader.activate(3, 10, 0, other_tab(3), &[1, 2], None, Spread::Single);
    reader.set_viewport(view(0.0));
    let report = draws(&reader);
    assert_eq!(report["owed_tab"], 2, "{report}");
    assert!(report["owed_pages"].as_u64().unwrap() <= 12, "{report}");
    assert_eq!(report["ended"]["replaced"], 1, "{report}");
    let did = run_jobs(&mut reader, &[]);
    assert!(did.iter().all(|job| job.tab != 1), "nothing for the one before: {did:?}");
    assert!(did.iter().any(|job| job.tab == 2 && job.kind == "preview"), "{did:?}");
}

/// Failure: a heavy page gets a preview drawn for its sleep (a heavy page's cost is per pass, which is why it has none on screen), or something's drawn for this while the system wants memory back.
#[test]
fn a_heavy_page_and_memory_pressure_get_nothing_drawn() {
    let mut reader = left_undrawn();
    let owed = reader.sleep_material(1, &map()).unwrap().in_view;
    let heavy: Vec<(u32, u32)> = owed.iter().map(|page| (1, *page)).collect();
    let did = run_jobs(&mut reader, &heavy);
    assert!(!did.iter().any(|job| job.tab == 1 && job.kind == "preview"), "{did:?}");
    assert_eq!(draws(&reader)["ended"]["done"], 1);

    let mut reader = left_undrawn();
    reader.memory_pressure(false);
    assert!(run_jobs(&mut reader, &[]).iter().all(|job| job.tab == 2));
    assert_eq!(draws(&reader)["ended"]["pressure"], 1, "{}", draws(&reader));
    // And a tab left owing while it's quiet after asks nothing.
    reader.note_version(Path::new("/d/2.pdf"), 1);
    reader.set_viewport(view(5_000.0));
    reader.activate(3, 10, 0, other_tab(3), &[1, 2], None, Spread::Single);
    assert_eq!(draws(&reader)["leaves_owing"], 1, "{}", draws(&reader));
    assert!(!SleepDraws::under_way(&reader));
}

/// Failure: the step asks again for a page that needs nothing more, or past what it may take, or for a tab it shouldn't draw for. Pure, page by page.
#[test]
fn the_step_asks_only_for_what_is_owed_and_within_its_bounds() {
    let held = TabNow {
        held: true,
        ..TabNow::default()
    };
    let page = |sized, heavy| PageNow {
        sized,
        heavy,
        ..PageNow::default()
    };
    let mut pages = vec![1, 2, 3, 4, 5];
    let look = |at: u32| match at {
        1 => PageNow {
            has_picture: true,
            ..page(true, Some(false))
        },
        2 => page(true, Some(true)),
        3 => PageNow {
            issue: true,
            ..page(true, Some(false))
        },
        4 => page(false, None),
        _ => page(true, Some(false)),
    };
    assert_eq!(
        next(held, &mut pages, 0, look),
        Next::Size(4),
        "pictured, heavy and failing pages are passed over"
    );
    assert_eq!(pages, vec![4, 5]);
    assert_eq!(
        next(held, &mut pages, 0, |at| if at == 4 { page(true, None) } else { look(at) }),
        Next::Weight(4)
    );
    assert_eq!(
        next(held, &mut pages, 0, |at| if at == 4 { page(true, Some(false)) } else { look(at) }),
        Next::Preview(4)
    );
    assert_eq!(next(held, &mut pages, MAX_JOBS, look), Next::End(Ended::OutOfJobs));
    assert_eq!(next(held, &mut Vec::new(), 0, look), Next::End(Ended::Done));
    for (tab, why) in [
        (TabNow { hot: true, ..held }, Ended::Hot),
        (TabNow { held: false, ..held }, Ended::Cold),
        (TabNow { problem: true, ..held }, Ended::Cold),
        (TabNow { pressure: true, ..held }, Ended::Pressure),
        (
            TabNow {
                picture_bytes: MAX_LEFT_PICTURE_BYTES,
                ..held
            },
            Ended::Full,
        ),
    ] {
        assert_eq!(next(tab, &mut vec![5], 0, look), Next::End(why), "{tab:?}");
    }
}

/// The outcome a pretend renderer gives a job, every page the size `size_of` says.
fn answer(job: &Job, size_of: impl Fn(u32) -> PageBox) -> Outcome {
    match &job.kind {
        JobKind::Size => Outcome::Size(size_of(job.position)),
        JobKind::Traits => Outcome::Heavy(false),
        JobKind::Text { .. } => Outcome::TextFailed,
        JobKind::Links => Outcome::Links(None),
        JobKind::Draw { .. } => Outcome::Drawn {
            kind: TileKind::Colour,
            low_res: None,
        },
    }
}

/// Failure: a tile or a region still being drawn for the tab just left (a heavy page's region can run seconds, in a worker of up to a gigabyte) went on to its deadline for nobody, kept as wanted because that tab was owed pictures. Only a size, a weight or a preview, what's asked for its place, is.
#[test]
fn a_tile_for_the_tab_just_left_is_cancelled_even_while_it_is_owed() {
    // Tab 2 drawn, so coming back to it wants nothing.
    let mut reader = Reader::new();
    reader.set_viewport(view(0.0));
    reader.activate(2, 10, 0, other_tab(2), &[], None, Spread::Single);
    reader.set_viewport(view(0.0));
    settle(&mut reader);
    reader.activate(1, 40, 0, map(), &[2], None, Spread::Single);
    reader.set_viewport(view(0.0));
    reader.note_version(Path::new("/d/a.pdf"), 1);
    settle(&mut reader);
    reader.set_viewport(view(20_000.0));
    // Sizes there first, then, its pages there being heavy, a region of one, with no preview under it.
    let region = loop {
        reader.host.take_ops();
        reader.applied();
        let job = reader.next_job().expect("tab 1 wants its new place drawn");
        reader.note_started(&job, None);
        if matches!(
            job.kind,
            JobKind::Draw {
                what: DrawKind::Tile | DrawKind::Region,
                ..
            }
        ) {
            break job;
        }
        let outcome = if matches!(job.kind, JobKind::Traits) {
            Outcome::Heavy(true)
        } else {
            answer(&job, |_| letter())
        };
        reader.finish(job, outcome);
    };
    reader.activate(2, 10, 0, other_tab(2), &[1], None, Spread::Single);
    reader.set_viewport(view(0.0));
    assert_eq!(draws(&reader)["owed_tab"], 1, "tab 1 is owed its place, page {} among it", region.position);
    assert!(!reader.view_needs_something(), "tab 2's drawn: only being owed could keep it");
    assert!(!reader.running_is_wanted(), "a region for a tab off screen is wanted by nobody");
}

/// Failure: a draw for a tab left started in the moment between a change of tab and the host's first word of the new one's view (no height yet, or its jump not arrived), when the tab showing had nothing it could ask for only because it didn't know where it was. A size can't be stopped once started, so the person's view waited on it.
#[test]
fn nothing_is_drawn_for_a_tab_left_before_the_tab_showing_is_placed() {
    // Tab 2, of one page, drawn: once it's where it's sent it wants nothing, so only being placed can hold the draw back.
    let mut reader = Reader::new();
    reader.set_viewport(view(0.0));
    reader.activate(2, 1, 0, other_tab(2), &[], None, Spread::Single);
    reader.set_viewport(view(0.0));
    settle(&mut reader);
    reader.activate(1, 40, 0, map(), &[2], None, Spread::Single);
    reader.set_viewport(view(0.0));
    reader.note_version(Path::new("/d/a.pdf"), 1);
    settle(&mut reader);
    reader.set_viewport(view(20_000.0));
    reader.activate(2, 1, 0, other_tab(2), &[1], None, Spread::Single);
    assert_eq!(draws(&reader)["owed_tab"], 1);
    let job = reader.next_job();
    assert!(
        job.as_ref().is_none_or(|job| job.tab == 2),
        "the jump to tab 2's place hasn't arrived: {:?}",
        job.map(|job| job.tab)
    );
    reader.set_viewport(Viewport { height: 0.0, ..view(0.0) });
    let job = reader.next_job();
    assert!(
        job.as_ref().is_none_or(|job| job.tab == 2),
        "a view with no height: {:?}",
        job.map(|job| job.tab)
    );
    reader.set_viewport(view(0.0));
    assert!(
        run_jobs(&mut reader, &[]).iter().any(|job| job.tab == 1),
        "and once it's placed, tab 1's is drawn"
    );
}

/// Failure: the pages drawn for a tab left aren't the pages its wake shows. Its place was taken to the pixel in a layout that the sizes learned for it since have moved, so what was drawn was a stretch of the document it isn't at. With pages of two sizes, what's drawn is what a return to it shows.
#[test]
fn the_pages_drawn_for_a_tab_left_are_the_pages_its_wake_shows() {
    // Page 20 turns out an eighth the height it was taken to be. Left with a sliver of it at the top, the rest of the view on page 21: once its size is known, there's too little of it in that view to be owed a picture.
    let size_of = |page: u32| if page == 20 { PageBox { width: 612.0, height: 100.0 } } else { letter() };
    let mut reader = reader_on(40);
    reader.note_version(Path::new("/d/a.pdf"), 1);
    settle(&mut reader);
    let page = reader.docs.get(&1).unwrap().layout.rect(20);
    reader.set_viewport(view(page.y + page.height * 0.92));
    assert_eq!(
        reader.sleep_material(1, &map()).unwrap().in_view,
        vec![20, 21],
        "both owed as it was taken to be"
    );
    let left_on = reader.place.last_view_page.expect("a page it's on");
    reader.activate(2, 10, 0, other_tab(2), &[1], None, Spread::Single);
    reader.set_viewport(view(0.0));
    let mut previews = Vec::new();
    for _ in 0..10_000 {
        reader.host.take_ops();
        reader.applied();
        let Some(job) = reader.next_job() else { break };
        if job.tab == 1 && matches!(job.kind, JobKind::Draw { what: DrawKind::Preview, .. }) {
            previews.push(job.position);
        }
        reader.note_started(&job, None);
        let outcome = answer(&job, size_of);
        reader.finish(job, outcome);
    }
    assert!(!previews.is_empty(), "{}", draws(&reader));
    // Back to it, warm, where its wake sends it.
    reader.activate(1, 40, left_on, map(), &[2], None, Spread::Single);
    let y = reader
        .host
        .take_ops()
        .into_iter()
        .find_map(|op| if let Op::ScrollTo { y, .. } = op { Some(y) } else { None })
        .expect("sent to its place");
    reader.set_viewport(view(y));
    let doc = reader.docs.get(&1).unwrap();
    let visible: Vec<u32> = reader.visible_pages(doc).collect();
    let shown = in_view_of(doc, &visible, y, 900.0);
    previews.sort_unstable();
    assert_eq!(previews, shown, "drawn for its sleep, against what its wake shows");
}

/// A picture that holds `bytes`, of a page nobody's at.
fn picture_of_bytes(bytes: usize) -> RawPicture {
    RawPicture {
        page: 999,
        scale: 1.0,
        width: 1,
        height: 1,
        paper: PaperKind::White,
        bgra: vec![0; bytes],
    }
}

/// Failure: a preview drawn for a tab left whose pictures can't take it, a worker's time and a surface for something thrown away as it lands. What wouldn't fit under the cap isn't asked for.
#[test]
fn a_preview_that_would_not_fit_under_the_cap_is_not_drawn() {
    let mut reader = left_undrawn();
    let doc = reader.docs.get_mut(&1).unwrap();
    doc.drawing
        .left_pictures
        .get_or_insert_with(Vec::new)
        .push(picture_of_bytes(MAX_LEFT_PICTURE_BYTES - 1024));
    let did = run_jobs(&mut reader, &[]);
    assert!(!did.iter().any(|job| job.tab == 1 && job.kind == "preview"), "{did:?}");
    assert_eq!(draws(&reader)["ended"]["full"], 1, "{}", draws(&reader));
}

/// Failure: the system asking for memory back ended the asking but let its job run on, a worker's memory held for a picture that would be let go.
#[test]
fn memory_pressure_stops_a_draw_for_a_tab_left_that_is_running() {
    let mut reader = left_undrawn();
    let job = settle_tab(&mut reader, 2).expect("a job for the tab left");
    assert_eq!(job.tab, 1);
    assert!(reader.running_is_wanted());
    reader.memory_pressure(false);
    assert!(reader.work.running.as_ref().is_some_and(|running| running.cancel_asked), "its cancel was asked");
}

/// Failure: a tab left whose pictures are at their cap, which can be drawn nothing more, took the place of another's asking that could still have been drawn.
#[test]
fn a_tab_left_with_no_room_for_a_picture_takes_no_other_tab_s_place() {
    let mut reader = left_undrawn();
    reader.note_version(Path::new("/d/2.pdf"), 1);
    reader.docs.get_mut(&2).unwrap().drawing.left_pictures = Some(vec![picture_of_bytes(MAX_LEFT_PICTURE_BYTES - 1024)]);
    SleepDraws::left(&mut reader, 2);
    let report = draws(&reader);
    assert_eq!(report["owed_tab"], 1, "{report}");
    assert_eq!(report["ended"]["replaced"], 0, "{report}");
    assert_eq!(report["ended"]["full"], 1, "{report}");
}

/// Failure: the previews drawn for a tab left push the tab showing's own previews out of the cache, so pages the person scrolls back to come back blank. They go first, the tab showing's stay.
#[test]
fn previews_drawn_for_a_tab_left_never_push_out_the_tab_showing_s() {
    let mut reader = Reader::new();
    reader.set_share(8);
    let previews_of = |reader: &Reader, tab: u32| reader.cache.entries.values().filter(|entry| entry.tab == tab && entry.preview).count();
    reader.set_viewport(view(0.0));
    reader.activate(2, 40, 0, other_tab(2), &[], None, Spread::Single);
    reader.note_version(Path::new("/d/2.pdf"), 1);
    for step in 0..30 {
        reader.set_viewport(view(f64::from(step) * 1_200.0));
        settle(&mut reader);
    }
    assert!(
        previews_of(&reader, 2) >= MAX_KEPT_PREVIEWS / 8,
        "tab 2 has its fill: {}",
        previews_of(&reader, 2)
    );
    reader.activate(1, 40, 0, map(), &[2], None, Spread::Single);
    reader.set_viewport(view(0.0));
    reader.note_version(Path::new("/d/a.pdf"), 1);
    settle(&mut reader);
    reader.set_viewport(view(20_000.0));
    reader.activate(2, 40, 0, other_tab(2), &[1], None, Spread::Single);
    reader.set_viewport(view(0.0));
    let job = settle_tab(&mut reader, 2).expect("a job for the tab left");
    let before = previews_of(&reader, 2);
    let outcome = answer(&job, |_| letter());
    reader.finish(job, outcome);
    run_jobs(&mut reader, &[]);
    assert!(draws(&reader)["pictures"].as_u64().unwrap() >= 1, "{}", draws(&reader));
    assert_eq!(previews_of(&reader, 2), before, "tab 2's previews all kept");
}

/// Failure: a left tab's asking keeps its worker from its idle exit for as long as it goes on. It's held to [`ASKING_FOR`] from its first job: a job at the limit is asked, one past it isn't.
#[test]
fn a_tab_left_is_asked_for_no_longer_than_its_time() {
    let mut reader = left_undrawn();
    let job = settle_tab(&mut reader, 2).expect("a job for the tab left");
    let outcome = answer(&job, |_| letter());
    reader.finish(job, outcome);
    let began = |reader: &mut Reader, ago: Duration| reader.wake.draws.owed.as_mut().unwrap().began = Some(Instant::now() - ago);
    began(&mut reader, ASKING_FOR - Duration::from_secs(1));
    let job = reader.next_job().expect("within its time");
    assert_eq!(job.tab, 1);
    reader.note_started(&job, None);
    let outcome = answer(&job, |_| letter());
    reader.finish(job, outcome);
    began(&mut reader, ASKING_FOR + Duration::from_millis(1));
    assert!(reader.next_job().is_none(), "past its time");
    assert_eq!(draws(&reader)["ended"]["out_of_time"], 1, "{}", draws(&reader));
}

/// Failure: a page found to need a heavy worker as it's drawn for a tab left ends the asking for the pages after it, which a normal worker can draw. It's passed over, counted, and the rest are drawn.
#[test]
fn a_page_that_would_need_a_heavy_worker_is_passed_over_and_the_rest_drawn() {
    // Left across the foot of page 20 and the head of page 21.
    let mut reader = reader_on(40);
    reader.note_version(Path::new("/d/a.pdf"), 1);
    settle(&mut reader);
    let page = reader.docs.get(&1).unwrap().layout.rect(20);
    reader.set_viewport(view(page.y + page.height - 450.0));
    reader.activate(2, 10, 0, other_tab(2), &[1], None, Spread::Single);
    reader.set_viewport(view(0.0));
    let owed = reader.sleep_material(1, &map()).unwrap().in_view;
    assert!(owed.len() >= 2, "{owed:?}");
    let mut drawn = Vec::new();
    for _ in 0..10_000 {
        reader.host.take_ops();
        reader.applied();
        let Some(job) = reader.next_job() else { break };
        reader.note_started(&job, None);
        let preview = job.tab == 1 && matches!(job.kind, JobKind::Draw { what: DrawKind::Preview, .. });
        let outcome = if preview && job.position == owed[0] {
            Outcome::Unstarted { heavy: true }
        } else {
            if preview {
                drawn.push(job.position);
            }
            answer(&job, |_| letter())
        };
        reader.finish(job, outcome);
    }
    assert_eq!(drawn, owed[1..], "{}", draws(&reader));
    assert_eq!(draws(&reader)["heavy_passed"], 1);
    assert_eq!(draws(&reader)["ended"]["done"], 1);
}

/// Failure: a page its map has no file for ended the asking as though the tab had gone cold, which it hadn't. Its own reason.
#[test]
fn a_page_with_no_file_ends_the_asking_with_its_own_reason() {
    let mut reader = left_undrawn();
    reader.docs.get_mut(&1).unwrap().map.slots = Some(Vec::new());
    assert!(run_jobs(&mut reader, &[]).iter().all(|job| job.tab == 2));
    assert_eq!(draws(&reader)["ended"]["no_source"], 1, "{}", draws(&reader));
    assert_eq!(draws(&reader)["ended"]["cold"], 0, "{}", draws(&reader));
}

/// Failure: whoever waits for drawing for a tab left waits for ever once the thread that draws has given up, as nothing more is drawn. It's not under way then.
#[test]
fn a_tab_left_is_not_under_way_once_drawing_has_given_up() {
    let mut reader = left_undrawn();
    assert!(SleepDraws::under_way(&reader));
    reader.thread_restarted(true);
    assert!(!SleepDraws::under_way(&reader), "{}", draws(&reader));
}

/// Failure: a warm tab whose pages change while it's off screen (its first deleted) is come back to without sleeping, and what's kept of it, the previews drawn for its place before it sleeps among them, goes up under the page numbers it had: pictures of other pages, for a frame or more. Everything of it on a layer once it's back was drawn after it came back.
#[test]
fn a_warm_tab_whose_pages_change_shows_nothing_kept_from_before() {
    let mut reader = left_undrawn();
    run_jobs(&mut reader, &[]);
    assert!(draws(&reader)["pictures"].as_u64().unwrap() >= 1, "{}", draws(&reader));
    let kept_before = reader
        .cache
        .entries
        .values()
        .filter(|entry| entry.tab == 1)
        .map(|entry| entry.id)
        .max()
        .expect("tab 1 has things kept");
    // Its first page gone: each position is the next page of the file.
    let shifted = PageMap {
        slots: Some((1..40).map(|page| (0, page)).collect()),
        ..map()
    };
    reader.activate(1, 39, 19, shifted, &[2], None, Spread::Single);
    reader.set_viewport(view(20_000.0));
    // Looked at after every step, as each is a frame the host can show.
    let mut shown_from_before = Vec::new();
    for _ in 0..10_000 {
        reader.host.take_ops();
        reader.applied();
        shown_from_before.extend(
            reader
                .cache
                .entries
                .values()
                .filter(|entry| entry.tab == 1 && entry.attached && entry.id <= kept_before)
                .map(|entry| entry.page),
        );
        let Some(job) = reader.next_job() else { break };
        reader.note_started(&job, None);
        let outcome = answer(&job, |_| letter());
        reader.finish(job, outcome);
    }
    assert!(
        shown_from_before.is_empty(),
        "kept from before its pages changed, on a layer: {shown_from_before:?}"
    );
}
