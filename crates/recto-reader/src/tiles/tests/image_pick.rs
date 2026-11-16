//! An image picked with a click ([`crate::image_pick`]), as the thread that draws is asked for it: the page's text, its hidden-text check and the tab's own
//! worker's boxes are jobs of that thread, and a click comes to an answer however they go.

use super::*;
use crate::image_pick::Under;

/// "find me here" near the top of a page (shown at 72 to 143 across, 82 to 92 down), read with the hidden-text check or without it.
fn line(checked: bool) -> Arc<recto_index::select::PageText> {
    let frame = recto_proto::PageFrame {
        turns: 0,
        left: 0.0,
        bottom: 0.0,
        right: 612.0,
        top: 792.0,
    };
    let chars: Vec<recto_proto::CharRecord> = "find me here".chars().map(|ch| recto_proto::CharRecord::new(ch as u32, 0)).collect();
    let boxes: Vec<recto_proto::CharBox> = (0..chars.len())
        .map(|at| [72.0 + at as f32 * 6.0, 700.0, 77.0 + at as f32 * 6.0, 710.0])
        .collect();
    Arc::new(recto_index::select::PageText::build(&chars, &boxes, frame, checked, false))
}

/// A still single click at `(x, y)` on page 1 as it's shown, as the mouse's press and release make it.
fn click(reader: &mut Reader, x: f64, y: f64) {
    let (x, y) = reader.docs.shown().and_then(|doc| doc.document_point(0, x, y)).expect("laid out");
    reader.selection_press(x, y, 1, false);
    reader.selection_release();
}

/// What the thread that draws is asked next for a click, everything else finished with a pretend renderer: a text read or an image boxes job, or none.
fn next_for_click(reader: &mut Reader) -> Option<Job> {
    loop {
        reader.host.take_ops();
        reader.applied();
        let job = reader.next_job()?;
        match &job.kind {
            JobKind::Text { .. } | JobKind::ImageBoxes { .. } => return Some(job),
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

/// Failure (S1 of the review: a click that picks nothing costs what it costs on `main`): the app is told to look for a click off text before the tab's own
/// worker has said an image is there, or after it said none is, so a margin costs a private worker; or the worker's boxes are asked for a click on text;
/// or a click whose page's text isn't held, or is held unchecked, waits for good ("Unknown always resolves"): on a heavy page, never drawn for the check,
/// and after a check tried and failed, its text is taken as showing. Each through the jobs the thread that draws is given, the view settled at the end.
#[test]
fn a_click_asks_the_text_then_the_workers_boxes_and_always_comes_to_an_answer() {
    let mut reader = reader_on(3);
    settle(&mut reader);

    // No text held: read first, then the worker's boxes, which say no image is there.
    click(&mut reader, 300.0, 400.0);
    let read = next_for_click(&mut reader).expect("the page's text");
    assert!(matches!(read.kind, JobKind::Text { check: false }) && read.position == 0, "{read:?}");
    reader.finish(
        read,
        Outcome::Text {
            text: line(true),
            checked: true,
        },
    );
    let boxes = next_for_click(&mut reader).expect("the worker's boxes");
    let JobKind::ImageBoxes { serial, at } = boxes.kind else {
        panic!("{boxes:?}")
    };
    assert_eq!(
        at,
        crate::image_pick::in_parts(recto_index::select::PagePoint { page: 0, x: 300.0, y: 400.0 }, (612.0, 792.0)).unwrap()
    );
    reader.finish(boxes, Outcome::ImageUnder(Under::No));
    assert!(!reader.pointer.take_image_news(), "no image there: the app isn't told to look");
    assert!(!reader.pointer.selecting.image.finding());

    // On text that shows: nothing's asked, of the text or the worker.
    click(&mut reader, 80.0, 87.0);
    assert!(next_for_click(&mut reader).is_none(), "on text that shows, nothing more is asked");
    assert!(!reader.pointer.selecting.image.finding());

    // On text held unchecked, a picture maybe under it: the worker first, then the check, and a check that failed takes the text as showing.
    reader.take_text(1, 0, Some((line(false), false)));
    click(&mut reader, 80.0, 87.0);
    let boxes = next_for_click(&mut reader).expect("the worker's boxes first");
    assert!(matches!(boxes.kind, JobKind::ImageBoxes { .. }), "{boxes:?}");
    reader.finish(boxes, Outcome::ImageUnder(Under::Yes));
    let check = next_for_click(&mut reader).expect("then the check");
    assert!(matches!(check.kind, JobKind::Text { check: true }), "{check:?}");
    reader.finish(
        check,
        Outcome::Text {
            text: line(false),
            checked: true,
        },
    );
    assert!(
        next_for_click(&mut reader).is_none() && !reader.pointer.selecting.image.finding(),
        "a failed check: text's"
    );
    assert!(!reader.pointer.take_image_news(), "and nothing looked for");

    // The same on a heavy page, with no check at all.
    reader.take_text(1, 0, Some((line(false), false)));
    reader.docs.views.get_mut(&1).unwrap().drawing.heavy.insert(0, true);
    click(&mut reader, 80.0, 87.0);
    assert!(
        next_for_click(&mut reader).is_none() && !reader.pointer.selecting.image.finding(),
        "a heavy page: text's"
    );

    // Off text, the worker saying an image is there: the app's told, once, and the view is settled.
    click(&mut reader, 300.0, 400.0);
    let boxes = next_for_click(&mut reader).expect("the worker's boxes");
    let JobKind::ImageBoxes { serial: newer, .. } = boxes.kind else {
        panic!("{boxes:?}")
    };
    assert_ne!(newer, serial, "each click its own serial");
    reader.finish(boxes, Outcome::ImageUnder(Under::Yes));
    assert!(reader.pointer.take_image_news(), "the app's told to look");
    assert!(!reader.pointer.take_image_news(), "once");
    assert!(next_for_click(&mut reader).is_none());
    assert!(reader.is_settled(), "nothing's left for the thread that draws");
}

/// Failure (the second review of `build/image-select`, fix 1): a click on text its page's first read hasn't checked for hidden text runs the check, a draw
/// of the whole page, though no picture is under it, which `main` never drew for a plain click. The tab's own worker is asked first, and with no picture
/// there nothing more is asked.
#[test]
fn a_click_on_text_with_no_picture_under_it_draws_no_check() {
    let mut reader = reader_on(3);
    settle(&mut reader);
    reader.take_text(1, 0, Some((line(false), false)));
    click(&mut reader, 80.0, 87.0);
    let first = next_for_click(&mut reader).expect("something asked");
    assert!(matches!(first.kind, JobKind::ImageBoxes { .. }), "the worker's boxes, not the check: {first:?}");
    reader.finish(first, Outcome::ImageUnder(Under::No));
    assert!(next_for_click(&mut reader).is_none(), "no picture under it: no check");
    assert!(!reader.pointer.selecting.image.finding() && !reader.pointer.take_image_news());
}

/// Failure (the second review of `build/image-select`, its blocker): a boxes job the worker couldn't answer (the page withheld or failing, its file
/// changed or refused, the renderer stopped) is taken as "an image may be there", so a click on such a page starts a private worker, the costliest on a
/// page whose drawing crashed, which will most likely crash the same way; or a stopped one is taken as either answer and not asked again. Stopped, it's
/// asked again; refused, the click picks nothing and the app isn't told to look, and the page isn't marked as failing for it.
#[test]
fn a_boxes_job_stopped_is_asked_again_and_one_refused_picks_nothing() {
    let mut reader = reader_on(3);
    settle(&mut reader);
    reader.take_text(1, 0, Some((line(true), true)));
    click(&mut reader, 300.0, 400.0);
    let boxes = next_for_click(&mut reader).expect("the worker's boxes");
    reader.finish(boxes, Outcome::Cancelled);
    let again = next_for_click(&mut reader).expect("asked again");
    assert!(matches!(again.kind, JobKind::ImageBoxes { .. }), "{again:?}");
    reader.finish(
        again,
        Outcome::PageFailed {
            message: "withheld".into(),
            may_pass: false,
        },
    );
    assert!(!reader.pointer.take_image_news(), "refused: nothing looked for");
    assert!(!reader.pointer.selecting.image.finding(), "and nothing waits");
    assert!(
        reader.docs.shown().unwrap().drawing.issues.is_empty(),
        "and the page isn't marked as failing for it"
    );
}
