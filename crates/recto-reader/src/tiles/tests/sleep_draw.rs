use super::*;
use crate::tiles::sleep_draw::{Ended, MAX_JOBS, Next, PageNow, SleepDraws, TabNow, next};
use crate::tiles::wake::MAX_LEFT_PICTURE_BYTES;

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
    let owed = reader.sleep_material(1, None).unwrap().in_view;
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
    let material = reader.sleep_material(1, None).unwrap();
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
    settle_tab(&mut reader, 2);
    assert!(SleepDraws::under_way(&reader));
    SleepDraws::no_worker(&mut reader);
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
    let owed = reader.sleep_material(1, None).unwrap().in_view;
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
