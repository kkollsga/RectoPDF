#![cfg(test)]
//! **The step of healing as the thread that draws takes it**, with a real render worker and no app: `at_rest` and what wakes it, on a page view the test settles itself (no thread draws).

use super::heal::landed;
use super::*;
use crate::engine::run::{OpenDoc, rest_or_wait_for_a_surface};
use crate::engine::{Engine, EngineConfig, EngineEvents};
use crate::layout::PageBox;
use crate::paper::PaperKind;
use crate::tiles::{JobKind, Op, Outcome, PageMap, Viewport};
use recto_render_service::{Policy, TileKind};
use std::path::PathBuf;

struct Quiet;
impl EngineEvents for Quiet {
    fn ops_ready(&self) {}
    fn note_changed(&self) {}
    fn page_moved(&self) {}
    fn page_drawn(&self, _tab: u32, _page: u32, _failure: Option<&'static str>, _asked: u64) {}
}

fn view() -> Viewport {
    Viewport {
        x: 0.0,
        y: 0.0,
        width: 1168.0,
        height: 900.0,
        frame_width: 1168.0,
        magnification: 1.0,
        backing: 2.0,
        live: false,
        scrolled_to: u64::MAX,
    }
}

/// What a test sees of the host: the ops it was told, with the surfaces given back as the app would.
fn told(engine: &Engine) -> Vec<Op> {
    let mut all = Vec::new();
    loop {
        let ops = engine.drain();
        if ops.is_empty() {
            break;
        }
        for op in &ops {
            if let Op::Attach { surface, .. } = op {
                recto_platform::surface::PixelSurface::release_for_host(*surface);
            }
        }
        all.extend(ops);
    }
    engine.applied();
    all
}

/// Every job the page view plans, drawn as a pretend renderer would (a page is letter-sized, a draw is of one colour), until there's none.
fn settle(engine: &Engine) {
    loop {
        let mut reader = crate::engine::lock(&engine.shared.reader);
        let Some(job) = reader.next_plan().and_then(crate::tiles::JobPlan::ready) else {
            break;
        };
        reader.work.note_started(&job, None);
        let outcome = match &job.kind {
            JobKind::Size => Outcome::Size(PageBox { width: 612.0, height: 792.0 }),
            JobKind::Traits => Outcome::Heavy(false),
            JobKind::Links => Outcome::Links(None),
            JobKind::Draw { .. } => Outcome::Drawn {
                kind: TileKind::Colour,
                low_res: None,
            },
            _ => Outcome::Cancelled,
        };
        reader.finish(job, outcome);
        drop(reader);
        told(engine);
    }
}

struct Rig {
    engine: Engine,
    file: recto_testpdf::TempPdf,
    renderers: HashMap<PathBuf, OpenDoc>,
}

/// Dark paper, tab 1 (warm, `V`) and tab 2 (shown, `W`) both on one file of one page, every tile of both drawn and told, and a renderer for the file with its worker running and page 0 kept for want of an answer, due now.
fn rig(keyed_for: Option<u32>) -> Rig {
    let file = recto_testpdf::TempPdf::new(
        "heal.pdf",
        &recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&[
            "A page whose look was kept",
            "for want of an answer",
        ]))]),
    );
    let engine = Engine::new(EngineConfig::default(), Arc::new(Quiet));
    engine.shut_down();
    let map = || PageMap {
        files: vec![file.0.clone()],
        ..PageMap::default()
    };
    engine.set_paper(PaperKind::Dark);
    for (tab, warm) in [(1, &[][..]), (2, &[1][..])] {
        engine.activate(tab, 1, 0, map(), warm, None, crate::layout::Spread::Single);
        engine.set_viewport(view());
        settle(&engine);
    }
    told(&engine);
    let mut open = OpenDoc::opened_for_test(&file.0, keyed_for);
    open.looks.keep(0, PageLook::undecided(false));
    open.looks.healing.insert(0, Instant::now());
    let renderers = HashMap::from([(file.0.clone(), open)]);
    Rig { engine, file, renderers }
}

fn tiles_of(ops: &[Op], page: u32) -> (usize, usize) {
    (
        ops.iter().filter(|op| matches!(op, Op::Attach { page: of, .. } if *of == page)).count(),
        ops.iter().filter(|op| matches!(op, Op::Detach { .. })).count(),
    )
}

/// Failure (privacy, review round 1): a heal drew the page of a tab whose pages are withheld (its unsaved edits don't fit the file), because the swap called `do_job` without the check every job meets. A withheld tab shown, a warm tab on the same file with the page kept for want of an answer: the step draws nothing of the page for the withheld tab (no picture of it is put on a layer), asks nothing of the file, and the look is not lifted. Seen failing with the check skipped (the preview of the withheld page attached).
#[test]
fn a_heal_draws_nothing_of_a_withheld_tab_s_page() {
    let mut rig = rig(None);
    crate::tiles::withhold_own_pages(&mut crate::engine::lock(&rig.engine.shared.reader), 2, true);
    told(&rig.engine);
    assert!(at_rest(&rig.engine.shared, &mut rig.renderers), "a refusal is a step");
    let ops = told(&rig.engine);
    assert_eq!(tiles_of(&ops, 0).0, 0, "nothing of the withheld page is drawn: {ops:?}");
    assert!(
        rig.renderers[&rig.file.0].looks.decided.contains_key(&0),
        "and the kept look is still kept, asked again later"
    );
}

/// Failure (privacy): a heal drew with a renderer keyed to a password typed for another tab, which no job of the shown tab may use. A renderer opened with the key of tab 1 doesn't serve tab 2, shown: nothing is drawn for it. Seen failing with the check skipped (the preview attached).
#[test]
fn a_heal_does_not_draw_by_a_renderer_keyed_to_another_tab() {
    let mut rig = rig(Some(1));
    assert!(at_rest(&rig.engine.shared, &mut rig.renderers));
    let ops = told(&rig.engine);
    assert_eq!(tiles_of(&ops, 0).0, 0, "nothing drawn by a renderer that isn't the tab's: {ops:?}");
}

/// Failure: the thread comes round at rest and a kept look is never asked again, or the heal is a draw that lands and leaves the old tiles beside the new preview. A page kept for want of an answer and due: the step asks, the preview is drawn by the new look and put on the page as the old tiles go (the host is told the detaches and the one attach), and the page is never asked again.
#[test]
fn a_due_page_heals_at_rest_and_the_old_tiles_go_as_the_preview_lands() {
    let mut rig = rig(None);
    assert!(at_rest(&rig.engine.shared, &mut rig.renderers), "a step was made");
    let ops = told(&rig.engine);
    let (attached, detached) = tiles_of(&ops, 0);
    assert_eq!(attached, 1, "the new preview: {ops:?}");
    assert!(detached >= 1, "and the old tiles off: {ops:?}");
    assert!(rig.renderers[&rig.file.0].looks.healing.is_empty(), "never healed again");
}

/// Failure (a blank page): the old look's tiles were dropped whatever the preview's draw came to, so a draw that failed left the page bare. A draw that doesn't land drops nothing, keeps the old look and asks again after [`HEAL_AFTER`]. Seen failing with the tiles dropped always (a `Detach` for a failed draw).
#[test]
fn a_preview_that_does_not_land_leaves_the_old_tiles_and_the_old_look() {
    let mut rig = rig(None);
    let now = Instant::now();
    let planned = crate::tiles::relook::stand_in_plan(&mut crate::engine::lock(&rig.engine.shared.reader), &rig.file.0, 0);
    let job = planned.and_then(crate::tiles::JobPlan::ready).expect("a preview is at hand");
    let open = rig.renderers.get_mut(&rig.file.0).unwrap();
    let Healed::Asked(kept) = heal(&mut open.doc, &mut open.looks, 0, 1, now) else {
        panic!("asked")
    };
    landed(
        &rig.engine.shared,
        open,
        job,
        kept,
        Outcome::PageFailed {
            message: "the Mac was busy".to_string(),
            may_pass: true,
        },
        now,
    );
    let ops = told(&rig.engine);
    assert_eq!(tiles_of(&ops, 0), (0, 0), "nothing dropped or put: {ops:?}");
    let open = &rig.renderers[&rig.file.0];
    assert!(open.looks.decided.contains_key(&0), "the old look is back");
    assert_eq!(open.looks.healing.get(&0), Some(&(now + HEAL_AFTER)), "asked again later");
}

/// Failure (the wiring, two ways to lose the heal): the thread at rest never takes the step (`at_rest` not called), or never wakes for a page whose time hasn't come (the soonest heal left out of the wait), so a kept look is asked again only when something else happens to wake it. With a page due in 300 ms, the thread's rest with nothing planned comes back by itself near that time, not at once and not never (a helper wakes it at 3 s so a thread that would sleep for ever fails, not hangs), and its next rest takes the step: the old tiles go and the preview lands. Seen failing with `heal` dropped from the rest's `retry` (still asleep at 2.5 s) and with the `at_rest` call removed (the tiles never go).
#[test]
fn the_thread_at_rest_wakes_for_a_kept_look_and_takes_the_step() {
    let mut rig = rig(None);
    let due = Instant::now() + Duration::from_millis(300);
    rig.renderers.get_mut(&rig.file.0).unwrap().looks.healing.insert(0, due);
    let sleeper = std::thread::current();
    let (finished, finish) = std::sync::mpsc::channel::<()>();
    let helper = std::thread::spawn(move || {
        // Wakes the test's thread at 3 s unless the test is over by then.
        if finish.recv_timeout(Duration::from_secs(3)).is_err() {
            sleeper.unpark();
        }
    });
    let (mut resting_since, began) = (None, Instant::now());
    let seen = rig.engine.shared.round_begins();
    rest_or_wait_for_a_surface(false, seen, &rig.engine.shared, &mut rig.renderers, &Policy::default(), &mut resting_since);
    let slept = began.elapsed();
    assert!(
        slept >= Duration::from_millis(150) && slept < Duration::from_millis(2500),
        "woke for the heal's time: {slept:?}"
    );
    assert!(told(&rig.engine).is_empty(), "nothing taken before its time");
    rest_or_wait_for_a_surface(false, seen, &rig.engine.shared, &mut rig.renderers, &Policy::default(), &mut resting_since);
    let ops = told(&rig.engine);
    assert_eq!(tiles_of(&ops, 0).0, 1, "the step was taken: {ops:?}");
    drop(finished);
    helper.join().expect("the helper ends");
}
