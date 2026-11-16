//! **An image picked with a click in read mode, as the person uses it** (owner, 2026-11-16: "when in normal read mode (with left click to select text)
//! clicking a image will show a selection box. Clicking cmd+c will copy lossless to clipboard."). Each click goes as the bridge's mouse calls make it: the
//! page view's press and release, then the window's `LinkClicked`. The page view decides whether to look, with jobs of its own thread (the page's text,
//! its hidden-text check, the tab's own worker's image boxes); the look, the person's `images --at`, is the one private worker a click may cost.

use super::files::locked_bytes;
use super::printing::session_held;
use crate::core::controls::UiAction;
use crate::core::image_select::{self, LET_GO_ID};
use crate::core::menu_rows::{self, ForAgent};
use crate::core::snapshot_tool;
use crate::testkit::headless::{Disk, HeadlessApp, WAIT_LIMIT, applied_as_the_host, host_turns};
use recto_agent::Level;
use serde_json::Value;
use std::path::Path;

/// The page's two images: a 2 x 2 RGB figure, so what's copied is a lossless PNG, and a 1 x 1 grey patch, stretched flat, as a scan of plain paper is.
fn images() -> Vec<recto_testpdf::Object> {
    let image = |size: u32, data: Vec<u8>| recto_testpdf::Object::Stream {
        dict: format!("/Type /XObject /Subtype /Image /Width {size} /Height {size} /ColorSpace /DeviceRGB /BitsPerComponent 8"),
        data,
    };
    vec![image(2, vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0]), image(1, vec![200, 200, 200])]
}

/// The figure's box on the page as it's shown, from its top left: 200 x 150 points at 100, 242.
const FIGURE: [f32; 4] = [100.0, 242.0, 200.0, 150.0];
/// The grey patch's: 200 x 60 points at 100, 582.
const PATCH: [f32; 4] = [100.0, 582.0, 200.0, 60.0];
/// On the figure, with no text there.
const ON_FIGURE: (f32, f32) = (250.0, 320.0);
/// On the figure too, elsewhere in its box.
const ELSEWHERE_ON_FIGURE: (f32, f32) = (150.0, 280.0);
/// On the caption drawn over the figure, which shows.
const ON_CAPTION: (f32, f32) = (150.0, 380.0);
/// On words drawn invisibly over the grey patch, which is flat, so the page's hidden-text check finds nothing drawn under them that the words would hide
/// and takes them for hidden.
const ON_HIDDEN: (f32, f32) = (150.0, 614.0);
/// On the line of body text at the top of the page.
const ON_TEXT: (f32, f32) = (100.0, 70.0);
/// Off any image and any text: a blank margin.
const OFF: (f32, f32) = (450.0, 600.0);

/// A page with a line of text at the top, the figure placed by `drawn` (a matrix and `Do`), a caption that shows drawn over the figure's foot, and the grey
/// patch with hidden words over it; `page_extra` for the page's dictionary, and `more` pages of plain text after it.
fn figure_page(drawn: &str, page_extra: &str, more: usize) -> Vec<u8> {
    let first = recto_testpdf::first_extra_object(1 + more);
    let content = [
        recto_testpdf::text_lines(&["Words at the top of the page"]),
        format!("q {drawn} Q\n").into_bytes(),
        b"BT /F1 12 Tf 110 410 Td (A caption over the figure) Tj ET\n".to_vec(),
        b"q 200 0 0 60 100 150 cm /Grey Do Q\n".to_vec(),
        b"BT 3 Tr /F1 12 Tf 110 175 Td (Hidden words of a scan) Tj ET\n".to_vec(),
    ]
    .concat();
    let mut pages = vec![recto_testpdf::Page {
        page_extra: page_extra.into(),
        ..recto_testpdf::Page::new(content)
    }];
    pages.extend((0..more).map(|at| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("Page {}", at + 2)]))));
    recto_testpdf::pages_with(&pages, &format!("/XObject << /Im {first} 0 R /Grey {} 0 R >>", first + 1), images())
}

fn figure() -> Vec<u8> {
    figure_page("200 0 0 150 100 400 cm /Im Do", "", 0)
}

/// The view a window would have, so the pages show and the box is told to the host.
fn shown(app: &HeadlessApp) {
    let mut view = recto_reader::Viewport {
        x: 0.0,
        y: 0.0,
        width: 1168.0,
        height: 900.0,
        frame_width: 1168.0,
        magnification: 1.0,
        backing: 2.0,
        live: false,
        scrolled_to: u64::MAX,
    };
    applied_as_the_host(&app.core().engine.set_viewport(view));
    host_turns(app, &mut view, true);
}

/// An app with `doc` open and showing.
fn opened(disk: Disk, doc: &Path) -> HeadlessApp {
    let app = disk.start();
    app.open(doc);
    shown(&app);
    app
}

/// A press at `from` with this many clicks, a drag to `to`, and the release, as the bridge's mouse calls make them with the snapshot tool off, then the
/// window's `LinkClicked`; nothing waited for. What the host was told of the image's box on the way, `ImageBox` ops as `(page, on)`.
fn let_go(app: &HeadlessApp, from: (f32, f32), to: Option<(f32, f32)>, clicks: u32) -> Vec<(u32, bool)> {
    let inner = app.core();
    let engine = inner.panes.reading_engine(&inner.engine);
    let at = |(x, y): (f32, f32)| engine.points().document_point(0, f64::from(x), f64::from(y)).expect("page 1 is laid out");
    let boxes = |ops: &[recto_reader::tiles::Op]| -> Vec<(u32, bool)> {
        applied_as_the_host(ops);
        ops.iter()
            .filter_map(|op| match op {
                recto_reader::tiles::Op::ImageBox { page, rect } => Some((*page, rect.is_some())),
                _ => None,
            })
            .collect()
    };
    let (x, y) = at(from);
    let mut told = boxes(&engine.selection().press(x, y, clicks, false));
    if let Some(to) = to {
        let (x, y) = at(to);
        told.extend(boxes(&engine.selection().drag(x, y)));
    }
    told.extend(boxes(&engine.selection().release()));
    app.act(UiAction::LinkClicked { pane: 0 });
    told
}

/// Whether any page view has a click waiting for its image, at whatever stage.
fn finding(app: &HeadlessApp) -> bool {
    app.core()
        .panes
        .engines(&app.core().engine)
        .iter()
        .any(|engine| engine.selection().image_finding())
}

/// Every click decided, every look landed, and the app at rest.
fn landed(app: &HeadlessApp) {
    app.wait_until("the clicks decided and their looks landed", || !finding(app) && !app.core().tools.tool().busy());
    app.settle();
}

/// A still single click, and what it looks for landed.
fn click(app: &HeadlessApp, at: (f32, f32)) {
    let_go(app, at, None, 1);
    landed(app);
}

/// What `qa state` says of the image picked: `reader.selection.image`.
fn image(app: &HeadlessApp) -> Value {
    app.core().panes.reading_engine(&app.core().engine).report().0["selection"]["image"].clone()
}

/// The image picked, its number and box, rounded to a hundredth of a point.
fn picked(app: &HeadlessApp) -> Option<(u32, [f32; 4])> {
    let (_, pick) = app.core().panes.reading_engine(&app.core().engine).selection().image_picked()?;
    let round = |value: f32| (value * 100.0).round() / 100.0;
    Some((pick.number, [pick.rect.x, pick.rect.y, pick.rect.width, pick.rect.height].map(round)))
}

/// Failure (owner, 2026-11-16): a click on an image in read mode picks nothing, or another image than the snapshot tool's click and an agent's `images
/// --at` find there, or no box is told to the host; a click on a caption that shows over the figure picks the figure, so its words can't be selected
/// with a click; a click off any image, or a double-click, picks one. And, of hidden text: words drawn invisibly (`3 Tr`) over the grey patch, a single
/// pixel stretched flat, are found hidden by the page's hidden-text check, as nothing under them differs from what they'd cover, and a click on them picks
/// the patch. That holds only for a flat picture: over a real one the check finds ink under the words and keeps them text's
/// (`known_gap_a_click_on_a_scans_ocr_word_over_a_real_picture_is_texts`).
#[test]
fn a_click_picks_the_image_under_it_and_text_that_shows_keeps_its_click() {
    let disk = Disk::new();
    let doc = disk.file("figure.pdf", &figure());
    let app = opened(disk, &doc);
    click(&app, ON_FIGURE);
    assert_eq!(picked(&app), Some((1, FIGURE)), "the figure, as the page shows it");
    assert_eq!(image(&app)["box_on_page"], 1, "its box told to the host: {}", image(&app));
    let agents = app.recto(Level::ReadOnly, &["images", "1", "--at", "250,320"]).expect("images --at");
    assert_eq!(
        (agents["images"][0]["number"].as_u64(), agents["images"][0]["rect"].clone()),
        (Some(1), serde_json::json!(FIGURE)),
        "the image an agent's look finds there"
    );

    click(&app, ON_CAPTION);
    assert_eq!(
        (picked(&app), image(&app)["box_on_page"].clone()),
        (None, Value::Null),
        "a caption that shows is text's"
    );
    click(&app, ON_HIDDEN);
    assert_eq!(picked(&app), Some((2, PATCH)), "hidden words don't keep the image under them from a click");
    click(&app, OFF);
    assert_eq!(picked(&app), None, "a click off any image");

    let_go(&app, ON_FIGURE, None, 1);
    let_go(&app, ON_FIGURE, None, 2);
    landed(&app);
    assert_eq!(picked(&app), None, "a double-click is text's, as it was");
    assert_eq!(image(&app)["finding"], false);
}

/// The person's `images` commands the window sent while `act` ran, once everything it started has landed: each is one private worker opened, the one
/// thing a click can cost past the page view's own jobs.
fn looks_sent(app: &HeadlessApp, act: impl FnOnce(&HeadlessApp)) -> usize {
    app.keep_sent();
    act(app);
    landed(app);
    app.sent().iter().filter(|command| command.tag() == "images").count()
}

/// Failure (S1 of the review of `build/image-select`: a click that picks nothing must cost what it costs on `main`): a still click on body text, on a
/// blank margin, the first click of a double click, or a click inside the box of the image already picked starts a private worker (351 MiB and 0.8 s on
/// the USGS map), where the page's text and the tab's own worker could say there's no image to look for; or a click on an image starts more than one; or a
/// click inside the box held takes the box off for its release to put back, a flicker. Counted as the person's `images` commands sent, each one worker.
#[test]
fn a_click_starts_a_private_worker_only_where_an_image_may_be() {
    let disk = Disk::new();
    let doc = disk.file("figure.pdf", &figure());
    let app = opened(disk, &doc);
    assert_eq!(
        looks_sent(&app, |app| drop(let_go(app, ON_TEXT, None, 1))),
        0,
        "body text, its page's text read and checked first"
    );
    assert_eq!(
        looks_sent(&app, |app| drop(let_go(app, ON_TEXT, None, 1))),
        0,
        "body text, its page's text held"
    );
    assert_eq!(looks_sent(&app, |app| drop(let_go(app, OFF, None, 1))), 0, "a blank margin");
    let double = |app: &HeadlessApp| {
        let_go(app, ON_TEXT, None, 1);
        let_go(app, ON_TEXT, None, 2);
    };
    assert_eq!(looks_sent(&app, double), 0, "a double click on text");
    assert_eq!(looks_sent(&app, |app| drop(let_go(app, ON_FIGURE, None, 1))), 1, "an image: one look");
    assert_eq!(picked(&app), Some((1, FIGURE)));
    let mut told = Vec::new();
    assert_eq!(
        looks_sent(&app, |app| told = let_go(app, ELSEWHERE_ON_FIGURE, None, 1)),
        0,
        "a click inside the box held"
    );
    assert_eq!(picked(&app), Some((1, FIGURE)), "still picked");
    assert!(told.is_empty(), "its box neither taken off nor put back: {told:?}");
}

/// Failure (S2 of the review): a click's look runs on to its limit after the person has moved on from the click, a private worker of 351 MiB on the USGS
/// map for nothing, holding a quit or an update's hand-off and every later look behind it: after Escape, the snapshot tool turned on, another tab, an edit
/// that moves the page, a newer click, or a quit. With the look held as it begins, each of these stops it, and it ends without its image being picked.
#[test]
fn a_look_is_stopped_when_the_person_moves_on() {
    let disk = Disk::new();
    let doc = disk.file("figure.pdf", &figure_page("200 0 0 150 100 400 cm /Im Do", "", 1));
    let other = disk.file("other.pdf", &figure());
    let app = opened(disk, &doc);
    let tool = || app.core().tools.tool();
    let stopped_by = |how: &str, move_on: &dyn Fn(&HeadlessApp)| {
        let (held, go) = app.hold_next("look_drawing");
        let_go(&app, ON_FIGURE, None, 1);
        held.recv_timeout(WAIT_LIMIT).expect("the look held as it begins");
        assert_eq!(tool().look_stopped(), Some(false), "{how}: the look runs");
        move_on(&app);
        app.wait_until(how, || tool().look_stopped() == Some(true));
        drop(go);
        landed(&app);
        assert_eq!(picked(&app), None, "{how}: nothing picked");
    };
    stopped_by("Escape", &|app| {
        app.core().perform(LET_GO_ID.into());
    });
    stopped_by("the snapshot tool turned on", &|app| {
        app.core().perform(snapshot_tool::TOGGLE_ID.into());
    });
    app.core().perform(snapshot_tool::TOGGLE_ID.into());
    stopped_by("an edit that moves the page", &|app| {
        app.agent(Level::Control, r#"{"cmd":"page_move","tab":"active","pages":"2","to":1}"#)
            .expect("a move");
    });
    app.agent(Level::Control, r#"{"cmd":"undo"}"#).expect("undone");
    shown(&app);
    stopped_by("a newer click, off any image", &|app| drop(let_go(app, OFF, None, 1)));
    stopped_by("a quit", &|app| app.core().tools.tool().quitting());
    tool().quit_not_taken();
    // Opened as the window's Open is, with nothing waited for: a settle would wait for the look held here.
    stopped_by("another tab", &|app| {
        app.act(UiAction::Open {
            path: other.display().to_string(),
        });
    });
}

/// Failure ("the last thing a person asked for wins over a read still landing"; rule 3): a click waits for its look, so the window stops while the
/// session's busy; or a look that lands after the person has moved on (a drag over text, another click) puts the image back. With the session held, the
/// click returns at once; its look lands after a drag and nothing's picked; and of three clicks while a look is under way, the newest is the one picked.
#[test]
fn a_look_landing_late_doesnt_bring_back_what_the_person_moved_on_from() {
    let disk = Disk::new();
    let doc = disk.file("figure.pdf", &figure());
    let app = opened(disk, &doc);
    let (release, holder) = session_held(&app);
    let_go(&app, ON_FIGURE, None, 1);
    app.wait_until("its look under way, the click having returned with the session held", || {
        app.core().tools.tool().busy()
    });
    let_go(&app, (72.0, 70.0), Some((250.0, 72.0)), 1);
    drop(release);
    holder.join().expect("the holder");
    landed(&app);
    assert_eq!(picked(&app), None, "the drag came after the click");

    let (release, holder) = session_held(&app);
    let_go(&app, OFF, None, 1);
    let_go(&app, ON_HIDDEN, None, 1);
    let_go(&app, ON_FIGURE, None, 1);
    drop(release);
    holder.join().expect("the holder");
    landed(&app);
    assert_eq!(
        app.core()
            .panes
            .reading_engine(&app.core().engine)
            .selection()
            .image_picked()
            .map(|(_, pick)| pick.at),
        Some(ON_FIGURE),
        "the newest click's"
    );
}

/// The person's commands the window sent while `act` ran, as their lines, once they've landed.
fn sent(app: &HeadlessApp, act: impl FnOnce(&HeadlessApp)) -> Vec<String> {
    app.keep_sent();
    act(app);
    app.wait_until("the snapshot's answered", || !app.core().tools.tool().busy());
    app.settle();
    app.sent().iter().map(recto_agent::Command::wire).collect()
}

/// What the clipboard is offered now, by type, and the tool's notice.
fn offered(app: &HeadlessApp) -> (Vec<String>, Option<String>) {
    let inner = app.core();
    let notice = inner.tools.tool().notice().map(|notice| notice.text);
    let types = inner.tools.tool().take_offer(&inner.panes.engines(&inner.engine)).map(|offer| offer.types);
    (types.unwrap_or_default(), notice)
}

/// Failure (coordinator, 2026-11-16: Copy isn't implemented a second time): ⌘C with an image picked sends something other than the snapshot tool's click
/// on it (another command, another point, another page as seen), or offers other types or other words; or Copy is enabled with nothing picked. The same
/// image copied both ways sends the same command and offers the same.
#[test]
fn copy_with_an_image_picked_is_the_snapshot_tools_click_on_it() {
    let disk = Disk::new();
    let doc = disk.file("figure.pdf", &figure());
    let app = opened(disk, &doc);
    assert!(!app.core().menu_item_enabled("edit.copy".into()), "nothing picked, nothing to copy");
    click(&app, ON_FIGURE);
    assert!(app.core().menu_item_enabled("edit.copy".into()), "an image picked is a selection to copy");
    let copied = sent(&app, |app| {
        app.core().perform("edit.copy".into());
    });
    let copy_offered = offered(&app);
    assert_eq!(
        copy_offered,
        (vec!["public.png".to_string()], Some("Copied the image, lossless, 2 × 2 pixels.".to_string()))
    );

    app.core().perform(snapshot_tool::TOGGLE_ID.into());
    app.settle();
    let tool_sent = sent(&app, |app| {
        let inner = app.core();
        let engine = inner.panes.reading_engine(&inner.engine);
        let (x, y) = engine.points().document_point(0, 250.0, 320.0).expect("laid out");
        let _ = inner.tools.tool().press(&engine, 0, x, y);
        let _ = inner.tools.tool().release(&engine, 0);
        app.act(UiAction::SnapshotTaken);
    });
    assert_eq!(copied, tool_sent, "the same command as the snapshot tool's click");
    assert!(copied.len() == 1 && copied[0].contains(r#""cmd":"image""#), "{copied:?}");
    assert_eq!(offered(&app), copy_offered, "and the same offer and words");
}

/// Failure (should-fix 3 of the review): text's "Copy anyway" (`edit.copy_anyway`, the text notice's button) copies an image picked, as the image's plain
/// Copy, so the text notice's button does the image's copy and the person's decision about a file's text goes to an image. Only the plain Copy takes the
/// image; text's Copy anyway sends no image command.
#[test]
fn texts_copy_anyway_never_copies_an_image_picked() {
    let disk = Disk::new();
    let doc = disk.file("figure.pdf", &figure());
    let app = opened(disk, &doc);
    click(&app, ON_FIGURE);
    let copied = sent(&app, |app| {
        app.core().perform("edit.copy_anyway".into());
    });
    assert!(!copied.iter().any(|line| line.contains(r#""cmd":"image""#)), "{copied:?}");
}

/// Failure (owner, 2026-11-08, for every copy): an image picked in a file that asks not to be copied goes to the clipboard without a word, or its notice
/// has no Copy anyway, or Copy anyway takes nothing. Picking it is no copy, and is allowed.
#[test]
fn a_file_that_asks_not_to_be_copied_gets_the_notice_and_copy_anyway_takes_it() {
    let disk = Disk::new();
    let doc = disk.file("restricted.pdf", &locked_bytes(&figure(), "", lopdf::encryption::Permissions::PRINTABLE));
    let app = opened(disk, &doc);
    click(&app, ON_FIGURE);
    assert_eq!(picked(&app), Some((1, FIGURE)), "picked: picking isn't copying");
    app.core().perform("edit.copy".into());
    app.wait_until("the copy's answered", || !app.core().tools.tool().busy());
    app.settle();
    let notice = app.core().tools.tool().selection_notice().expect("a notice");
    assert_eq!(
        (notice.text.as_str(), notice.action_title.as_deref()),
        ("This file's permissions ask that it isn't copied.", Some("Copy anyway"))
    );
    assert_eq!(offered(&app).0, Vec::<String>::new(), "nothing offered");
    app.act(UiAction::Menu {
        id: notice.action_id.expect("its button"),
    });
    app.wait_until("Copy anyway's answered", || !app.core().tools.tool().busy());
    app.settle();
    assert_eq!(offered(&app).0, ["public.png"], "Copy anyway takes it");
}

/// Picks the figure, asserting it's picked.
fn pick(app: &HeadlessApp) {
    click(app, ON_FIGURE);
    assert_eq!(picked(app), Some((1, FIGURE)), "picked");
}

/// Failure (coordinator, 2026-11-16): an image stays picked, or its box on the page, after the person has moved on: a click off it, a drag over text,
/// Select All, Escape, the snapshot tool turned on or Capture chosen, an edit that moves its page, another tab; or Escape is taken from whatever else
/// wants it with nothing picked. A turn, which moves no page, keeps it, as it keeps a text selection.
#[test]
fn an_image_picked_is_let_go_by_whatever_the_person_does_next() {
    let disk = Disk::new();
    let doc = disk.file("figure.pdf", &figure_page("200 0 0 150 100 400 cm /Im Do", "", 1));
    let other = disk.file("other.pdf", &figure());
    let app = opened(disk, &doc);
    let gone = |app: &HeadlessApp, how: &str| {
        app.settle();
        assert_eq!((picked(app), image(app)["box_on_page"].clone()), (None, Value::Null), "{how}");
    };
    pick(&app);
    click(&app, OFF);
    gone(&app, "a click off any image");

    pick(&app);
    let_go(&app, (72.0, 70.0), Some((250.0, 72.0)), 1);
    landed(&app);
    gone(&app, "a drag over text");
    assert!(app.core().panes.reading_engine(&app.core().engine).selection().has_any(), "which selects it");

    pick(&app);
    app.core().perform("edit.select_all".into());
    gone(&app, "Select All");

    pick(&app);
    app.core().perform(LET_GO_ID.into());
    gone(&app, "Escape");

    pick(&app);
    app.core().perform(snapshot_tool::TOGGLE_ID.into());
    gone(&app, "the snapshot tool turned on");
    app.core().perform(snapshot_tool::TOGGLE_ID.into());

    pick(&app);
    app.core().perform(snapshot_tool::CAPTURE_ID.into());
    gone(&app, "Capture chosen");
    app.core().perform(snapshot_tool::CANCEL_ID.into());

    pick(&app);
    app.agent(Level::Control, r#"{"cmd":"page_rotate","tab":"active","pages":"1","degrees":90}"#)
        .expect("a turn");
    shown(&app);
    assert_eq!(
        (picked(&app), image(&app)["box_on_page"].clone()),
        (Some((1, FIGURE)), Value::from(1)),
        "a turn moves no page, and the box turns with its page, as a text selection does"
    );
    app.agent(Level::Control, r#"{"cmd":"page_move","tab":"active","pages":"2","to":1}"#)
        .expect("a move");
    gone(&app, "an edit of the pages");
    app.agent(Level::Control, r#"{"cmd":"undo"}"#).expect("undone");
    app.agent(Level::Control, r#"{"cmd":"undo"}"#).expect("undone");
    shown(&app);

    pick(&app);
    app.open(&other);
    shown(&app);
    gone(&app, "another tab");
}

/// What Escape does now, with nothing held: the id it's sent as, or none.
fn escape(app: &HeadlessApp) -> Option<&'static str> {
    image_select::plain_key(&app.core().panes, &app.core().engine, app.core().tools.tool(), "\u{1b}")
}

/// Failure (S3 of the review): Escape lets an image picked go while a Capture waits, so the Capture's Escape is lost; or, with no Capture, Escape doesn't
/// let the image go; or, with neither, Escape is claimed and taken from whatever else wants it. A Capture waiting with an image picked (made here by the
/// tool's own call: Capture from its menu item lets the image go): the first Escape ends the Capture, the second lets the image go, the third is nobody's.
#[test]
fn escape_ends_a_capture_first_then_lets_the_image_go() {
    let disk = Disk::new();
    let doc = disk.file("figure.pdf", &figure());
    let app = opened(disk, &doc);
    assert_eq!(escape(&app), None, "neither: Escape isn't claimed");
    pick(&app);
    assert!(app.core().tools.tool().arm_once(), "a Capture waits");
    assert_eq!(escape(&app), Some(snapshot_tool::CANCEL_ID), "the Capture's first");
    app.core().perform(snapshot_tool::CANCEL_ID.into());
    assert!(
        !app.core().tools.tool().is_on() && picked(&app).is_some(),
        "the Capture ended, the image still picked"
    );
    assert_eq!(escape(&app), Some(LET_GO_ID), "then the image's");
    app.core().perform(LET_GO_ID.into());
    app.settle();
    assert_eq!(picked(&app), None, "let go");
    assert_eq!(escape(&app), None, "and then nobody's");
}

/// Opens a protected file with its password, as the person types it into the sheet.
fn open_locked(app: &HeadlessApp, path: &Path) {
    app.act(UiAction::Open {
        path: path.display().to_string(),
    });
    app.settle();
    let prompt = app.reader().password_prompt.expect("its password is asked for");
    app.core()
        .tools
        .prompts()
        .password_answered(prompt.id, Some("secret".into()), &app.core().core, &app.core().window);
    app.settle();
    shown(app);
}

/// An agent's `images --at` at a point, naming the page as seen as the window's own look does.
fn as_the_window(app: &HeadlessApp, (x, y): (f32, f32)) -> String {
    let tab = app.core().panes.reading_engine(&app.core().engine).hot_tab().expect("a tab");
    format!(r##"{{"cmd":"images","tab":"#{tab}","page":1,"at":[{x},{y}],"as_seen":{{"quarter_turns":0,"file_page":0,"own":true}}}}"##)
}

/// Failure (AGENTS.md, "an agent never gets more of a protected document than it could see before", and "nothing an agent sends runs as the person"): an
/// agent's `images --at` reads a document the person opened with its password; an agent's look, at the very point of a click of the person's still being
/// looked for and naming the page as seen as the window's does, is taken for the window's look, so its answer picks the image in the person's window; or
/// an agent's run of Escape's id lets go of the person's image. The person's own click reads it with their password, and is picked by its own look.
#[test]
fn an_agent_gets_nothing_new_of_a_picked_image_or_a_protected_document() {
    let disk = Disk::new();
    let locked = disk.file("locked.pdf", &locked_bytes(&figure(), "secret", lopdf::encryption::Permissions::all()));
    let plain = disk.file("plain.pdf", &figure());
    let app = disk.start();
    open_locked(&app, &locked);
    click(&app, ON_FIGURE);
    assert_eq!(picked(&app), Some((1, FIGURE)), "the person's click reads it with their password");
    for level in [Level::ReadOnly, Level::Control, Level::Full] {
        let refused = app.recto(level, &["images", "1", "--at", "250,320"]).expect_err("an agent's look");
        assert_eq!(refused.code, recto_agent::Code::PROTECTED, "{level:?}: {}", refused.message);
    }
    assert!(
        app.agent(Level::Full, &as_the_window(&app, ON_FIGURE)).is_err(),
        "refused on the protected document"
    );

    app.open(&plain);
    shown(&app);
    let (held, go) = app.hold_next("look_drawing");
    let_go(&app, ON_FIGURE, None, 1);
    held.recv_timeout(WAIT_LIMIT).expect("the person's look held as it begins");
    let agents = app
        .agent(Level::ReadOnly, &as_the_window(&app, ON_FIGURE))
        .expect("an agent's look on a plain file, at the same point");
    assert_eq!(agents["images"][0]["number"], 1, "it finds the figure, for itself");
    // What the session thread was left to do of it done; a settle would wait for the person's look held here.
    assert!(app.core().core.flush(WAIT_LIMIT), "the session thread took its calls");
    assert_eq!(picked(&app), None, "the agent's answer isn't the person's look's: nothing picked by it");
    drop(go);
    landed(&app);
    assert_eq!(picked(&app).map(|(_, rect)| rect), Some(FIGURE), "the person's own look picks it");
    assert!(matches!(
        menu_rows::for_agent(app.core(), LET_GO_ID, |_, command| Ok(command)),
        Ok(ForAgent::PersonsOwn { .. })
    ));
    assert!(picked(&app).is_some(), "still picked");
}

/// Failure (rule 6): an image drawn far past its page gives the host a box past the page, or one drawn absurdly large (its numbers past what a point
/// can be) panics the look or is picked. Held to the page, the one partly past it is picked with its box cut to the page; the ones drawn past the whole page
/// are the page, and aren't picked (owner, 2026-11-16); the absurd ones aren't listed, as the snapshot tool's click and `recto images` don't list them.
#[test]
fn an_image_drawn_far_past_its_page_is_held_to_it() {
    let disk = Disk::new();
    let drawn = "2000 0 0 2000 -500 -500 cm /Im Do Q q 1e6 0 0 1e6 -5e5 -5e5 cm /Im Do Q q 1e39 0 0 1e39 0 0 cm /Im Do Q q 400 0 0 400 300 -100 cm /Im Do";
    let doc = disk.file("far.pdf", &figure_page(drawn, "", 0));
    let app = opened(disk, &doc);
    click(&app, OFF);
    assert_eq!(picked(&app), Some((4, [300.0, 492.0, 312.0, 300.0])), "the figure's box cut to the page");
    assert_eq!(image(&app)["box_on_page"], 1);
}

/// Failure: on a page shown turned, by an unsaved turn or by the file's own, a click picks the image beside the one under it, or the tab's own worker's
/// boxes are compared at the wrong place so the click looks for nothing, or its box isn't the one `recto images` gives, which the page's layer turns as it
/// turns the page. The click goes in at the middle of the box `images` lists, through the page view's own sums for a turned page.
#[test]
fn on_a_turned_page_the_click_picks_the_image_under_it() {
    for (how, bytes, turn) in [
        ("a turn of the file's own", figure_page("200 0 0 150 100 400 cm /Im Do", "/Rotate 90", 0), false),
        ("an unsaved turn", figure(), true),
    ] {
        let disk = Disk::new();
        let doc = disk.file("turned.pdf", &bytes);
        let app = opened(disk, &doc);
        if turn {
            app.agent(Level::Control, r#"{"cmd":"page_rotate","tab":"active","pages":"1","degrees":90}"#)
                .expect("a turn");
            shown(&app);
        }
        let listed = app.recto(Level::ReadOnly, &["images", "1"]).expect("images");
        let rect: Vec<f32> = (0..4).map(|at| listed["images"][0]["rect"][at].as_f64().unwrap() as f32).collect();
        click(&app, (rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0));
        assert_eq!(picked(&app), Some((1, [rect[0], rect[1], rect[2], rect[3]])), "{how}");
        assert_eq!(image(&app)["box_on_page"], 1, "{how}: its box told");
    }
}

/// A 32 x 32 RGB picture whose every pixel differs from its neighbours, as a scan's is, for the known gap below.
fn noisy_picture() -> recto_testpdf::Object {
    let data: Vec<u8> = (0..32u32 * 32 * 3).map(|at| (at.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    recto_testpdf::Object::Stream {
        dict: "/Type /XObject /Subtype /Image /Width 32 /Height 32 /ColorSpace /DeviceRGB /BitsPerComponent 8".into(),
        data,
    }
}

/// A scan page: one picture over the whole page (`page_extra` for the page's dictionary, e.g. a turn), the OCR words invisibly over it when `ocr`, and a
/// noisy `figure` placed over it (200 x 200 at 100, 300 up the page: 100, 292 from the top as shown) when `figure`.
fn scan_page(ocr: bool, figure: bool, page_extra: &str) -> Vec<u8> {
    let mut content = b"q 612 0 0 792 0 0 cm /Scan Do Q\n".to_vec();
    if figure {
        content.extend(b"q 200 0 0 200 100 300 cm /Fig Do Q\n");
    }
    if ocr {
        content.extend(b"BT 3 Tr /F1 12 Tf 120 600 Td (Words a scan's OCR put here) Tj ET\n");
    }
    let first = recto_testpdf::first_extra_object(1);
    recto_testpdf::pages_with(
        &[recto_testpdf::Page {
            page_extra: page_extra.into(),
            ..recto_testpdf::Page::new(content)
        }],
        &format!("/XObject << /Scan {first} 0 R /Fig {} 0 R >>", first + 1),
        vec![noisy_picture(), noisy_picture()],
    )
}

/// Whether a text caret is placed (a selection's start, with nothing selected), as `qa state` says.
fn caret(app: &HeadlessApp) -> bool {
    let selection = app.core().panes.reading_engine(&app.core().engine).report().0["selection"].clone();
    selection["start"] != Value::Null && selection["selected"] == false
}

/// **A scan is never picked by a click** (owner, 2026-11-16: "lets treat true images and scanned pages differently. Click to select only for true
/// images."; it replaces the known gap this test held, where a click on an OCR word was text's and a click between words picked the scan). Failure: a
/// click on a scan, with an OCR layer or without, on a word, between the words or at the page's edge, picks the page or starts a private worker to find
/// out; or the click on a word stops placing the caret. A one-image scan costs 0 `images --at`, and on a word the caret is placed as before.
#[test]
fn a_click_on_a_scan_picks_nothing_and_costs_no_worker() {
    for ocr in [true, false] {
        let disk = Disk::new();
        let doc = disk.file("scan.pdf", &scan_page(ocr, false, ""));
        let app = opened(disk, &doc);
        // The OCR line is 600 to 612 up the page, 180 to 192 down it as it's shown.
        for (what, at) in [
            ("on a word's place", (160.0, 188.0)),
            ("between the words", (300.0, 400.0)),
            ("at the page's edge", (4.0, 4.0)),
        ] {
            let looks = looks_sent(&app, |app| drop(let_go(app, at, None, 1)));
            assert_eq!((looks, picked(&app)), (0, None), "ocr {ocr}, {what}");
            assert_eq!(image(&app)["box_on_page"], Value::Null, "ocr {ocr}, {what}: no box");
        }
        if ocr {
            let_go(&app, (160.0, 188.0), None, 1);
            landed(&app);
            assert!(caret(&app), "a click on an OCR word still places the caret");
        }
    }
}

/// **A figure placed over a scan is picked, the scan beside it isn't** (owner, 2026-11-16). Failure: the scan's page-wide box under the point hides the
/// figure from the filter, so a figure on a scanned page can't be picked; or the look's answer for a point on the scan (the scan, as it's on top there)
/// is picked. The click on the figure costs one look, the click on the scan beside it none, and `recto images --at` still names the scan there.
#[test]
fn a_figure_over_a_scan_is_picked_and_the_scan_beside_it_isnt() {
    let disk = Disk::new();
    let doc = disk.file("scan-figure.pdf", &scan_page(true, true, ""));
    let app = opened(disk, &doc);
    assert_eq!(
        looks_sent(&app, |app| drop(let_go(app, (200.0, 392.0), None, 1))),
        1,
        "a figure over a scan: one look"
    );
    assert_eq!(picked(&app), Some((2, [100.0, 292.0, 200.0, 200.0])), "the figure, not the scan under it");
    assert_eq!(looks_sent(&app, |app| drop(let_go(app, (500.0, 600.0), None, 1))), 0, "the scan beside it");
    assert_eq!(picked(&app), None);
    let agents = app.recto(Level::ReadOnly, &["images", "1", "--at", "500,600"]).expect("images --at");
    assert_eq!(agents["images"][0]["number"].as_u64(), Some(1), "the lookup still names the scan");
}

/// Failure (owner, 2026-11-16): on a turned page, by the file's own turn or an unsaved one, the scan's box is compared against the page unturned, so a
/// scan of a turned page is taken for a figure and picked, or a figure on a turned scan isn't. Clicks at the middle of each, by the page's own sums.
#[test]
fn on_a_turned_page_a_scan_isnt_picked_and_a_figure_over_it_is() {
    for (how, extra, turn) in [("a turn of the file's own", "/Rotate 90", false), ("an unsaved turn", "", true)] {
        let disk = Disk::new();
        let doc = disk.file("turned-scan.pdf", &scan_page(true, true, extra));
        let app = opened(disk, &doc);
        if turn {
            app.agent(Level::Control, r#"{"cmd":"page_rotate","tab":"active","pages":"1","degrees":90}"#)
                .expect("a turn");
            shown(&app);
        }
        let listed = app.recto(Level::ReadOnly, &["images", "1"]).expect("images");
        let figure = listed["images"]
            .as_array()
            .unwrap()
            .iter()
            .find(|image| image["number"] == 2)
            .expect("the figure listed");
        let rect: Vec<f32> = (0..4).map(|at| figure["rect"][at].as_f64().unwrap() as f32).collect();
        let on_scan = (rect[0] / 2.0, rect[1] / 2.0 + 1.0);
        assert_eq!(looks_sent(&app, |app| drop(let_go(app, on_scan, None, 1))), 0, "{how}: the scan");
        assert_eq!(picked(&app), None, "{how}: the scan");
        click(&app, (rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0));
        assert_eq!(picked(&app).map(|(number, _)| number), Some(2), "{how}: the figure");
    }
}

/// A page of 72 small pictures in a grid of 8 by 9, apart from each other, more than an answer of the worker's holds (64), drawn row by row from the
/// page's foot: picture `n` (from 1) is in column `(n - 1) % 8` and row `(n - 1) / 8`.
fn crowded() -> Vec<u8> {
    let content: String = (0..72)
        .map(|at| format!("q 30 0 0 30 {} {} cm /Im Do Q\n", 40 + (at % 8) * 60, 100 + (at / 8) * 70))
        .collect();
    let first = recto_testpdf::first_extra_object(1);
    recto_testpdf::pages_with(
        &[recto_testpdf::Page::new(content.into_bytes())],
        &format!("/XObject << /Im {first} 0 R >>"),
        vec![images().remove(0)],
    )
}

/// The middle of picture `n` of [`crowded`], as the page shows it from its top left.
fn on_crowded(n: usize) -> (f32, f32) {
    let at = n - 1;
    (40.0 + (at % 8) as f32 * 60.0 + 15.0, 792.0 - (100.0 + (at / 8) as f32 * 70.0) - 15.0)
}

/// Failure (the second review of `build/image-select`, its blocker): on a page of more pictures apart than the worker's answer holds, as a catalogue page of
/// photos is, the worker can't say where they are, so every click there, a margin's or a gap's, starts a private worker; or the answer that says is wrong
/// past its 64th, so a picture past them can't be picked. A margin click and a click between pictures send no look; a click on the 3rd, 10th and 70th
/// picture each picks that one, through one look each.
#[test]
fn on_a_page_of_more_pictures_than_an_answer_holds_a_margin_click_costs_nothing() {
    let disk = Disk::new();
    let doc = disk.file("catalogue.pdf", &crowded());
    let app = opened(disk, &doc);
    assert_eq!(looks_sent(&app, |app| drop(let_go(app, (20.0, 20.0), None, 1))), 0, "a margin");
    assert_eq!(
        looks_sent(&app, |app| drop(let_go(app, (40.0 + 45.0 + 7.0, 400.0), None, 1))),
        0,
        "a gap between pictures"
    );
    // The worker's walk keeps 64 and puts the rest in one box round them, its first row or its last as it meets them: a picture of each row is asked.
    for n in [3, 10, 70] {
        assert_eq!(looks_sent(&app, |app| drop(let_go(app, on_crowded(n), None, 1))), 1, "picture {n}: one look");
        assert_eq!(picked(&app).map(|(number, _)| number), Some(n as u32), "picture {n}");
    }
}

/// A contact sheet: 6 pictures spread to the page's corners and middle, drawn first, then 64 small ones in a grid of 8 by 8, apart. The worker's walk meets
/// the pictures from the top of the drawing down, so it holds the grid's 64 and the 65th box is the one round the 6 drawn first, which covers nearly the
/// whole page (all but a 2-point margin).
fn contact_sheet() -> Vec<u8> {
    let spread = [(3, 3), (579, 759), (300, 650), (360, 650), (420, 650), (480, 650)].into_iter();
    let grid = (0..64).map(|at| (40 + (at % 8) * 60, 100 + (at / 8) * 70));
    let content: String = spread.chain(grid).map(|(x, y)| format!("q 30 0 0 30 {x} {y} cm /Im Do Q\n")).collect();
    let first = recto_testpdf::first_extra_object(1);
    recto_testpdf::pages_with(
        &[recto_testpdf::Page::new(content.into_bytes())],
        &format!("/XObject << /Im {first} 0 R >>"),
        vec![images().remove(0)],
    )
}

/// Failure (the coordinator's review of `build/scan-not-picked`): the worker's 65th box, "something may be anywhere in here" past the 64 pictures it
/// holds, covers a contact sheet's page, and is taken for a scan, so a picture past the 64th can't be picked, where it could before the branch. A click on
/// the one at the top right, past the 64 held, picks it through one look; a click on the margin outside every box, and a plain one-image scan, send none.
#[test]
fn a_picture_past_the_64th_on_a_contact_sheet_is_still_picked() {
    let disk = Disk::new();
    let doc = disk.file("contact.pdf", &contact_sheet());
    let app = opened(disk, &doc);
    assert_eq!(
        looks_sent(&app, |app| drop(let_go(app, (1.0, 1.0), None, 1))),
        0,
        "the margin outside every box"
    );
    assert_eq!(
        looks_sent(&app, |app| drop(let_go(app, (594.0, 18.0), None, 1))),
        1,
        "a picture past the 64th: one look"
    );
    assert_eq!(picked(&app).map(|(_, rect)| rect), Some([579.0, 3.0, 30.0, 30.0]), "that picture");
    let disk = Disk::new();
    let doc = disk.file("scan.pdf", &scan_page(false, false, ""));
    let app = opened(disk, &doc);
    assert_eq!(looks_sent(&app, |app| drop(let_go(app, (300.0, 400.0), None, 1))), 0, "a plain scan");
}

/// Failure (the second review, fix 2): a copy while a click's look runs stops the look, and its stopped answer is taken as "no image there", so the click
/// picks nothing, though the look was to wait behind the copy. The look runs again after the copy, and picks its image.
#[test]
fn a_look_a_copy_stops_runs_again_after_the_copy() {
    let disk = Disk::new();
    let doc = disk.file("figure.pdf", &figure());
    let app = opened(disk, &doc);
    let looks = looks_sent(&app, |app| {
        let (held, go) = app.hold_next("look_drawing");
        let_go(app, ON_FIGURE, None, 1);
        held.recv_timeout(WAIT_LIMIT).expect("the look held as it begins");
        // The person's copy of the figure, as Copy with it picked or the snapshot tool's click on it sends it.
        let inner = app.core();
        let engine = inner.panes.reading_engine(&inner.engine);
        let seen = engine.points().page_seen(0).expect("page 1");
        let seen = recto_agent::extract::AsSeen {
            quarter_turns: seen.quarter_turns,
            file_page: seen.file_page,
            own: seen.own,
        };
        inner
            .tools
            .tool()
            .take_image(engine.hot_tab().expect("a tab"), 0, seen, [ON_FIGURE.0, ON_FIGURE.1]);
        snapshot_tool::taken(inner);
        app.wait_until("the look stopped for the copy", || inner.tools.tool().look_stopped() == Some(true));
        drop(go);
    });
    assert_eq!(looks, 2, "the look, and the look again after the copy");
    assert_eq!(picked(&app), Some((1, FIGURE)), "the click picks its image");
    assert_eq!(offered(&app).0, ["public.png"], "and the copy was made");
}

/// Failure (the second review, fix 3): a click on an image while the app quits starts a private worker, which a quit then waits for. On the way out no
/// look starts and the click picks nothing; staying, clicks look again.
#[test]
fn no_look_starts_while_the_app_quits() {
    let disk = Disk::new();
    let doc = disk.file("figure.pdf", &figure());
    let app = opened(disk, &doc);
    app.core().tools.tool().quitting();
    assert_eq!(looks_sent(&app, |app| drop(let_go(app, ON_FIGURE, None, 1))), 0, "no look on the way out");
    assert_eq!(picked(&app), None);
    app.core().tools.tool().quit_not_taken();
    pick(&app);
}

/// Failure (the second review, fix 5): a person's plain `images` (no click's point and page as seen) is handed the window's look's stop, or a stopped one
/// when no look runs, and is refused with "A newer click took its place". Answered as on `main`, a look running or not.
#[test]
fn a_persons_plain_images_isnt_taken_for_a_clicks_look() {
    let disk = Disk::new();
    let doc = disk.file("figure.pdf", &figure());
    let app = opened(disk, &doc);
    let plain = || {
        let tab = app.core().panes.reading_engine(&app.core().engine).hot_tab().expect("a tab");
        let command = recto_agent::parse(&format!(r##"{{"cmd":"images","tab":"#{tab}","page":1}}"##)).expect("images");
        app.core().host.run_for_person(&command, &mut Vec::new(), crate::queue::Origin::Input)
    };
    let listed = plain().expect("no look running: answered");
    assert_eq!(listed["count"], 2, "{listed}");
    let (held, go) = app.hold_next("look_drawing");
    let_go(&app, ON_FIGURE, None, 1);
    held.recv_timeout(WAIT_LIMIT).expect("the look held as it begins");
    assert_eq!(plain().expect("a look running: answered")["count"], 2);
    drop(go);
    landed(&app);
    assert_eq!(picked(&app), Some((1, FIGURE)), "and the look goes on as ever");
}

/// A page whose whole content is one form, drawn twice the size it's written at (`/Matrix [2 0 0 2 0 0]`), as a re-imposed, stamped or slide-deck page
/// is: inside it, a picture over the whole page (`/Bg`, number 1) and the figure (number 2), 200 x 150 points at 100, 242 from the top as shown.
fn page_in_a_form() -> Vec<u8> {
    let first = recto_testpdf::first_extra_object(1);
    let form = recto_testpdf::Object::Stream {
        dict: format!(
            "/Type /XObject /Subtype /Form /BBox [0 0 306 396] /Matrix [2 0 0 2 0 0] /Resources << /XObject << /Bg {} 0 R /Im {} 0 R >> >>",
            first + 1,
            first + 2
        ),
        data: b"q 306 0 0 396 0 0 cm /Bg Do Q q 100 0 0 75 50 200 cm /Im Do Q".to_vec(),
    };
    recto_testpdf::pages_with(
        &[recto_testpdf::Page::new(b"q /Fm Do Q".to_vec())],
        &format!("/XObject << /Fm {first} 0 R >>"),
        vec![form, noisy_picture(), images().remove(0)],
    )
}

/// Failure (the review of `build/scan-not-picked`, its first regression): the tab's own worker gives a picture inside a form the box of the outermost form,
/// which on a page wrapped in one page-wide form is the page, so the filter takes the figure for a scan and a click on it sends no look and picks nothing.
/// The figure's own box, through the form's matrix, is what the click is judged by: a click on it picks it through one look; a click on the page-wide
/// picture beside it, the page's own, sends none.
#[test]
fn a_figure_inside_a_page_wide_form_is_picked() {
    let disk = Disk::new();
    let doc = disk.file("in-a-form.pdf", &page_in_a_form());
    let app = opened(disk, &doc);
    assert_eq!(looks_sent(&app, |app| drop(let_go(app, ON_FIGURE, None, 1))), 1, "the figure: one look");
    assert_eq!(picked(&app), Some((2, FIGURE)), "the figure, by its own box");
    assert_eq!(looks_sent(&app, |app| drop(let_go(app, OFF, None, 1))), 0, "the page-wide picture beside it");
    assert_eq!(picked(&app), None);
}

/// A page of more objects than the worker's walk looks at (20,000): 20,001 small squares, then the figure, placed as [`figure`]'s is.
fn past_the_walks_cap() -> Vec<u8> {
    let mut content = b"0 0 0 rg\n".to_vec();
    content.extend(b"10 10 2 2 re f\n".repeat(20_001));
    content.extend(b"q 200 0 0 150 100 400 cm /Im Do Q\n");
    let first = recto_testpdf::first_extra_object(1);
    recto_testpdf::pages_with(
        &[recto_testpdf::Page::new(content)],
        &format!("/XObject << /Im {first} 0 R >>"),
        vec![images().remove(0)],
    )
}

/// Failure (the review of `build/scan-not-picked`, its second regression): a walk cut short by its cap on objects says a picture may be anywhere on the
/// page with a whole-page box, which on a page of fewer than 64 pictures was taken for a scan, so on a CAD or map page of more than 20,000 objects a click
/// on an inset picture drawn after them picks nothing. The click on it picks it through one look; a click on the margin, where the walk can't say there's
/// no picture, costs one look too (one private worker, one at a time and stopped by the next click, as every look is) and picks nothing.
#[test]
fn a_picture_past_the_walks_cap_is_picked() {
    let disk = Disk::new();
    let doc = disk.file("busy.pdf", &past_the_walks_cap());
    let app = opened(disk, &doc);
    assert_eq!(looks_sent(&app, |app| drop(let_go(app, ON_FIGURE, None, 1))), 1, "the picture: one look");
    assert_eq!(picked(&app), Some((1, FIGURE)));
    assert_eq!(
        looks_sent(&app, |app| drop(let_go(app, OFF, None, 1))),
        1,
        "the margin: one look, as the walk can't say"
    );
    assert_eq!(picked(&app), None);
}
