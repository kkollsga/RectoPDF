//! **An image picked with a click in read mode, as the person uses it** (owner, 2026-11-16: "when in normal read mode (with left click to select text)
//! clicking a image will show a selection box. Clicking cmd+c will copy lossless to clipboard."). Each click goes as the bridge's mouse calls make it: the
//! page view's press and release, then the window's `LinkClicked`, which looks for the image.

use super::files::locked_bytes;
use super::printing::session_held;
use crate::core::controls::UiAction;
use crate::core::image_select::{self, LET_GO_ID};
use crate::core::menu_rows::{self, ForAgent};
use crate::core::snapshot_tool;
use crate::testkit::headless::{Disk, HeadlessApp, applied_as_the_host, host_turns};
use recto_agent::Level;
use serde_json::Value;
use std::path::Path;

/// The page's two images: a 2 x 2 RGB figure, so what's copied is a lossless PNG, and a 1 x 1 grey patch, as a scan of plain paper is.
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
/// On the caption drawn over the figure, which shows.
const ON_CAPTION: (f32, f32) = (150.0, 380.0);
/// On words drawn invisibly over the grey patch, with no ink of their own or the patch's under them, so the page's hidden-text check finds them hidden.
const ON_HIDDEN: (f32, f32) = (150.0, 614.0);
/// Off any image.
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
/// window's `LinkClicked`; nothing waited for.
fn let_go(app: &HeadlessApp, from: (f32, f32), to: Option<(f32, f32)>, clicks: u32) {
    let inner = app.core();
    let engine = inner.panes.reading_engine(&inner.engine);
    let at = |(x, y): (f32, f32)| engine.points().document_point(0, f64::from(x), f64::from(y)).expect("page 1 is laid out");
    let (x, y) = at(from);
    applied_as_the_host(&engine.selection().press(x, y, clicks, false));
    if let Some(to) = to {
        let (x, y) = at(to);
        applied_as_the_host(&engine.selection().drag(x, y));
    }
    applied_as_the_host(&engine.selection().release());
    app.act(UiAction::LinkClicked { pane: 0 });
}

/// Every look landed and the app at rest.
fn landed(app: &HeadlessApp) {
    app.wait_until("the looks landed", || !app.core().tools.image_select().busy());
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
/// with a click; words the page's hidden-text check finds hidden keep the image under them from being picked; a click off any image, or a double-click,
/// picks one.
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
/// Select All, Escape, the snapshot tool turned on, an edit that moves its page, another tab; or Escape is taken from whatever else wants it with nothing
/// picked. A turn, which moves no page, keeps it, as it keeps a text selection.
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
    assert_eq!(
        image_select::plain_key(&app.core().panes, &app.core().engine, app.core().tools.tool(), "\u{1b}"),
        None,
        "Escape isn't this feature's with nothing picked"
    );
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
    let escape = image_select::plain_key(&app.core().panes, &app.core().engine, app.core().tools.tool(), "\u{1b}")
        .expect("Escape is this feature's with an image picked");
    assert_eq!(escape, LET_GO_ID);
    app.core().perform(escape.into());
    gone(&app, "Escape");

    pick(&app);
    app.core().perform(snapshot_tool::TOGGLE_ID.into());
    gone(&app, "the snapshot tool turned on");
    app.core().perform(snapshot_tool::TOGGLE_ID.into());

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

/// Failure ("the last thing a person asked for wins over a read still landing"; rule 3): a click waits for its look, so the window stops while the
/// session's busy; or a look that lands after the person has moved on (a drag over text, another click) puts the image back. With the session held, the
/// click returns at once; its look lands after a drag and nothing's picked; and of two clicks while a look is under way, the newest is the one picked.
#[test]
fn a_look_landing_late_doesnt_bring_back_what_the_person_moved_on_from() {
    let disk = Disk::new();
    let doc = disk.file("figure.pdf", &figure());
    let app = opened(disk, &doc);
    let (release, holder) = session_held(&app);
    let_go(&app, ON_FIGURE, None, 1);
    assert!(
        app.core().tools.image_select().busy(),
        "its look under way, and the click returned with the session held"
    );
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

/// Failure (AGENTS.md, "an agent never gets more of a protected document than it could see before"): an agent's `images --at` reads a document the
/// person opened with its password; an agent's look, which names the page as seen as the window's does, picks an image in the person's window; or an
/// agent's run of Escape's id lets go of the person's image. The person's own click reads it with their password.
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
    let as_the_window = |app: &HeadlessApp| {
        let tab = app.core().panes.reading_engine(&app.core().engine).hot_tab().expect("a tab");
        format!(r##"{{"cmd":"images","tab":"#{tab}","page":1,"at":[300.0,350.0],"as_seen":{{"quarter_turns":0,"file_page":0,"own":true}}}}"##)
    };
    assert!(app.agent(Level::Full, &as_the_window(&app)).is_err(), "refused on the protected document");
    app.open(&plain);
    shown(&app);
    click(&app, ON_FIGURE);
    app.agent(Level::ReadOnly, &as_the_window(&app)).expect("an agent's look on a plain file");
    app.settle();
    assert_eq!(picked(&app).map(|(_, rect)| rect), Some(FIGURE), "the person's pick, as it was");
    assert!(matches!(
        menu_rows::for_agent(app.core(), LET_GO_ID, |_, command| Ok(command)),
        Ok(ForAgent::PersonsOwn { .. })
    ));
    assert!(picked(&app).is_some(), "still picked");
}

/// Failure (rule 6): an image drawn far past its page gives the host a box past the page, or one drawn absurdly large (its numbers past what a point
/// can be) panics the look or is picked. Held to the page, the one past it is picked with the page's box; the absurd ones aren't listed, as the snapshot
/// tool's click and `recto images` don't list them either.
#[test]
fn an_image_drawn_far_past_its_page_is_held_to_it() {
    let disk = Disk::new();
    let drawn = "2000 0 0 2000 -500 -500 cm /Im Do Q q 1e6 0 0 1e6 -5e5 -5e5 cm /Im Do Q q 1e39 0 0 1e39 0 0 cm /Im Do";
    let doc = disk.file("far.pdf", &figure_page(drawn, "", 0));
    let app = opened(disk, &doc);
    click(&app, OFF);
    assert_eq!(picked(&app), Some((1, [0.0, 0.0, 612.0, 792.0])), "the page's box");
    assert_eq!(image(&app)["box_on_page"], 1);
}

/// Failure: on a page shown turned, by an unsaved turn or by the file's own, a click picks the image beside the one under it, or its box isn't the one
/// `recto images` gives, which the page's layer turns as it turns the page. The click goes in at the middle of the box `images` lists, through the
/// page view's own sums for a turned page.
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
