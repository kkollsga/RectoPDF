//! **The snapshot tool, as the person uses it:** a box dragged on a page, or a click on an image, and what reaches the clipboard (S2's follow-up (1), a guard
//! put in before the tool's snapshot moved into the one executor).

use super::files::{locked_allowing, locked_with};
use crate::core::controls::UiAction;
use crate::core::snapshot_tool::{self, ClipboardOffer};
use crate::testkit::headless::{Disk, HeadlessApp, WAIT_LIMIT};
use recto_agent::Level;
use recto_agent::extract::{self, ClipItem, ClipOutcome, ClipStop};
use std::path::Path;

/// The 4 x 4 red JPEG of `export_svg.rs`, as its own bytes.
const JPEG_HEX: &str = "ffd8ffe000104a46494600010100000100010000ffdb0043000302020302020303030304030304050805050404050a070706080c0a0c0c0b0a0b0b0d0e12100d0e110e0b0b1016101113141515150c0f171816141812141514ffdb00430103040405040509050509140d0b0d1414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414ffc00011080004000403012200021101031101ffc4001f0000010501010101010100000000000000000102030405060708090a0bffc400b5100002010303020403050504040000017d01020300041105122131410613516107227114328191a1082342b1c11552d1f02433627282090a161718191a25262728292a3435363738393a434445464748494a535455565758595a636465666768696a737475767778797a838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae1e2e3e4e5e6e7e8e9eaf1f2f3f4f5f6f7f8f9faffc4001f0100030101010101010101010000000000000102030405060708090a0bffc400b51100020102040403040705040400010277000102031104052131061241510761711322328108144291a1b1c109233352f0156272d10a162434e125f11718191a262728292a35363738393a434445464748494a535455565758595a636465666768696a737475767778797a82838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae2e3e4e5e6e7e8e9eaf2f3f4f5f6f7f8f9faffda000c03010002110311003f00f0aa28a2be68fedd3fffd9";

fn jpeg() -> Vec<u8> {
    (0..JPEG_HEX.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&JPEG_HEX[at..at + 2], 16).unwrap())
        .collect()
}

/// A page with a line of text and the JPEG over 100..140 across and 252..292 down, from the top left.
fn text_and_image() -> Vec<u8> {
    let image = recto_testpdf::first_extra_object(1);
    let content = [
        recto_testpdf::text_lines(&["Words to take in a box"]),
        b"q 40 0 0 40 100 500 cm /Im Do Q\n".to_vec(),
    ]
    .concat();
    let dict = "/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode".to_string();
    recto_testpdf::pages_with(
        &[recto_testpdf::Page::new(content)],
        &format!("/XObject << /Im {image} 0 R >>"),
        vec![recto_testpdf::Object::Stream { dict, data: jpeg() }],
    )
}

/// The box every case takes, on the page as the file shows it, in points from its top left: the line of text, and the image below it.
const BOX: [f32; 4] = [40.0, 30.0, 300.0, 280.0];

/// Turns the tool on, if it's off.
fn tool_on(app: &HeadlessApp) {
    if !app.core().tools.tool().is_on() {
        app.core().perform(snapshot_tool::TOGGLE_ID.into());
    }
    app.settle();
}

/// The document's point for a point on page 1, from its top left, as the page view has it.
fn at(app: &HeadlessApp, x: f32, y: f32) -> (f64, f64) {
    let engine = app.core().panes.reading_engine(&app.core().engine);
    engine.points().document_point(0, f64::from(x), f64::from(y)).expect("page 1 is laid out")
}

/// A press at `from`, a drag to `to`, and the release, as the bridge's mouse calls make them with the tool on; then the snapshot's job to its end.
fn mouse(app: &HeadlessApp, from: (f32, f32), to: Option<(f32, f32)>) {
    let_go(app, from, to);
    app.wait_until("the snapshot's job ended", || !app.core().tools.tool().busy());
    app.settle();
}

/// A press, a drag and the release, as [`mouse`], without waiting for what it takes.
fn let_go(app: &HeadlessApp, from: (f32, f32), to: Option<(f32, f32)>) {
    let inner = app.core();
    let engine = inner.panes.reading_engine(&inner.engine);
    let tool = inner.tools.tool();
    let (x, y) = at(app, from.0, from.1);
    let _ = tool.press(&engine, 0, x, y);
    if let Some(to) = to {
        let (x, y) = at(app, to.0, to.1);
        let _ = tool.drag(&engine, 0, x, y);
    }
    let _ = tool.release(&engine, 0);
    app.act(UiAction::SnapshotTaken);
}

/// A box drawn and let go, as [`take_box`], without waiting for what it takes.
fn let_go_of_a_box(app: &HeadlessApp) {
    let_go(app, (BOX[0], BOX[1]), Some((BOX[0] + BOX[2], BOX[1] + BOX[3])));
}

/// One type's bytes, as the bridge's `clipboard_data` gives them: made with the copy, or a promised one the person's paste makes, waited for.
fn clipboard_data(app: &HeadlessApp, serial: u64, uti: &str) -> Option<Vec<u8>> {
    let tool = app.core().tools.tool();
    if let Some(bytes) = tool.ready_bytes(serial, uti) {
        return Some(bytes);
    }
    app.act(UiAction::Pasted { serial, uti: uti.into() })
        .understood
        .then(|| tool.wait_for_paste())
        .flatten()
}

fn take_box(app: &HeadlessApp) {
    mouse(app, (BOX[0], BOX[1]), Some((BOX[0] + BOX[2], BOX[1] + BOX[3])));
}

/// The notice by the page, and its button, as a line.
fn notice(app: &HeadlessApp) -> String {
    match app.core().tools.tool().selection_notice() {
        Some(notice) => format!("{:?} [{}]", notice.text, notice.action_title.unwrap_or_default()),
        None => "-".into(),
    }
}

/// A type offered, and the bytes it gave when asked.
type Given = (String, Option<Vec<u8>>);

/// What the host would be offered now, and every type's bytes, a promise made as a paste would make it.
fn offered(app: &HeadlessApp) -> Option<(ClipboardOffer, Vec<Given>)> {
    let inner = app.core();
    let offer = inner.tools.tool().take_offer(&inner.panes.engines(&inner.engine))?;
    let bytes = offer.types.iter().map(|uti| (uti.clone(), clipboard_data(app, offer.serial, uti))).collect();
    Some((offer, bytes))
}

/// The types offered, in order, a promised one marked with `*`.
fn types(offer: &ClipboardOffer) -> String {
    let named: Vec<String> = offer
        .types
        .iter()
        .map(|uti| if offer.promised.contains(uti) { format!("{uti}*") } else { uti.clone() })
        .collect();
    format!("[{}]", named.join(", "))
}

/// What `extract` makes for this box of page 1, turned so far: the bytes each type of the tool's offer must be.
fn reference(path: &Path, turns: u8, anyway: bool) -> Vec<(String, Vec<u8>)> {
    let ClipOutcome::Ready(clip) = extract::snapshot_clip(path, 0, BOX, (turns, None), anyway, &ClipStop::default()) else {
        panic!("the reference snapshot")
    };
    let mut made: Vec<(String, Vec<u8>)> = clip
        .items
        .iter()
        .filter_map(|item| match item {
            ClipItem::Ready { uti, bytes } => Some(((*uti).to_string(), bytes.clone())),
            ClipItem::Promised { .. } => None,
        })
        .collect();
    let svg = extract::snapshot_svg(path, 0, BOX, turns, None, extract::MAX_CLIP_ITEM).expect("the reference SVG");
    made.push((extract::UTI_SVG.to_string(), svg));
    made
}

/// Whether every type offered is what `extract` makes for it: "same", or the types that differ.
fn compared(bytes: &[Given], reference: &[(String, Vec<u8>)]) -> String {
    let differ: Vec<&str> = bytes
        .iter()
        .filter(|(uti, made)| reference.iter().find(|(named, _)| named == uti).map(|(_, bytes)| bytes) != made.as_ref())
        .map(|(uti, _)| uti.as_str())
        .collect();
    if differ.is_empty() { "same".into() } else { format!("differ {differ:?}") }
}

/// One line for a box taken: the notice, what's offered, and whether each type is what `extract` makes for the page turned so far.
fn a_box(app: &HeadlessApp, path: &Path, turns: u8, anyway: bool) -> String {
    let said = notice(app);
    match offered(app) {
        Some((offer, bytes)) => format!("{said} | {} | {}", types(&offer), compared(&bytes, &reference(path, turns, anyway))),
        None => format!("{said} | nothing offered"),
    }
}

/// What a snapshot of the person's did, a line each, on 2026-11-14, before the tool's snapshot became the person's `snapshot` command.
const GOLDEN: &[&str] = &[
    "a box on a text page | \"Copied the snapshot as PNG, text, SVG and JPG.\" [] | [public.png, public.utf8-plain-text, public.svg-image*, public.jpeg] | same",
    "its text | \"Words to take in a box\"",
    "a click on a JPEG | \"Copied the image as its own JPEG, 4 × 4 pixels.\" [] | [public.jpeg, public.png] | its own bytes",
    "a click off any image | \"There's no image there. Drag a box to take part of the page.\" [] | nothing offered",
    "a box on dark paper | \"Copied the snapshot as PNG, text, SVG and JPG.\" [] | [public.png, public.utf8-plain-text, public.svg-image*, public.jpeg] | same",
    "a box on a page turned a quarter, unsaved | \"Copied the snapshot as PNG, text, SVG and JPG.\" [] | [public.png, public.utf8-plain-text, public.svg-image*, public.jpeg] | same",
    "its file changed on disk under an edit | \"That page can't be read now: its file has changed or gone.\" [] | nothing offered",
    "asks not to be copied | \"This file's permissions ask that it isn't copied.\" [Copy anyway] | nothing offered",
    "its Copy anyway | \"Copied the snapshot as PNG, text, SVG and JPG.\" [] | [public.png, public.utf8-plain-text, public.svg-image*, public.jpeg] | same",
    // Decided 2026-11-14 (S2's follow-up (1), Q5): the person's own snapshot reads a document they opened with its password, as the page view does; it
    // said "This PDF needs a password." before, and offered nothing. And its flag against copying is read with the password.
    "opened with its password | \"Copied the snapshot as PNG, text, SVG and JPG.\" [] | [public.png, public.utf8-plain-text, public.svg-image*, public.jpeg] | every type given | text \"Locked page 1\"",
    "opened with its password, asking not to be copied | \"This file's permissions ask that it isn't copied.\" [Copy anyway] | nothing offered",
];

/// **Failure (a guard before the snapshot tool's release became the person's `snapshot` and `image` commands):** what the tool puts on the clipboard, or
/// says by the page, changes: other types or another order, the SVG made at once rather than promised, bytes other than `extract` makes for the box (as
/// the page is seen, turned by an unsaved turn; the same on dark paper), a JPEG clicked not its own bytes, other words or another button for a click off any
/// image, a file changed under the tab, one that asks not to be copied and its Copy anyway, or a document opened with its password (read with it, its
/// flag against copying too). One app, each case in turn, held against the golden.
#[test]
fn the_snapshot_tool_does_what_it_did() {
    let disk = Disk::new();
    let doc = disk.file("snapshot.pdf", &text_and_image());
    let restricted = disk.file("restricted.pdf", &locked_allowing("", 1, lopdf::encryption::Permissions::PRINTABLE));
    let locked = disk.file("locked.pdf", &locked_with("secret", 1));
    let locked_restricted = disk.file(
        "locked-restricted.pdf",
        &locked_allowing("secret", 1, lopdf::encryption::Permissions::PRINTABLE),
    );
    let changing = disk.file("changing.pdf", &text_and_image());
    let app = disk.start();
    app.open(&doc);
    app.settle();
    tool_on(&app);
    let mut lines = Vec::new();

    take_box(&app);
    lines.push(format!("a box on a text page | {}", a_box(&app, &doc, 0, false)));
    let text = reference(&doc, 0, false)
        .into_iter()
        .find(|(uti, _)| uti == extract::UTI_TEXT)
        .map(|(_, bytes)| String::from_utf8(bytes).unwrap());
    lines.push(format!("its text | {:?}", text.unwrap_or_default()));

    mouse(&app, (120.0, 270.0), None);
    let said = notice(&app);
    let (offer, bytes) = offered(&app).expect("the image is offered");
    let own = bytes.first().and_then(|(_, bytes)| bytes.as_deref()) == Some(&jpeg()[..]);
    lines.push(format!(
        "a click on a JPEG | {said} | {} | {}",
        types(&offer),
        if own { "its own bytes" } else { "not its own bytes" }
    ));

    mouse(&app, (400.0, 600.0), None);
    lines.push(format!("a click off any image | {} | {}", notice(&app), offered_or_nothing(&app)));

    app.agent(Level::Control, r#"{"cmd":"view","paper":"dark"}"#).expect("dark paper");
    app.settle();
    take_box(&app);
    lines.push(format!("a box on dark paper | {}", a_box(&app, &doc, 0, false)));
    app.agent(Level::Control, r#"{"cmd":"view","paper":"white"}"#).expect("white paper");

    app.agent(Level::Control, r#"{"cmd":"page_rotate","tab":"active","pages":"all","degrees":90}"#)
        .expect("a turn");
    app.settle();
    take_box(&app);
    lines.push(format!("a box on a page turned a quarter, unsaved | {}", a_box(&app, &doc, 1, false)));

    // A tab with no edits goes by the file as it is now; one with an edit, by the file its pages were recorded from.
    open_with_tool(&app, &changing);
    app.agent(Level::Control, r#"{"cmd":"page_rotate","tab":"active","pages":"all","degrees":90}"#)
        .expect("a turn");
    app.settle();
    std::fs::write(
        &changing,
        recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["Other words"]))]),
    )
    .unwrap();
    take_box(&app);
    lines.push(format!(
        "its file changed on disk under an edit | {} | {}",
        notice(&app),
        offered_or_nothing(&app)
    ));

    open_with_tool(&app, &restricted);
    take_box(&app);
    lines.push(format!("asks not to be copied | {} | {}", notice(&app), offered_or_nothing(&app)));
    let anyway = app
        .core()
        .tools
        .tool()
        .selection_notice()
        .and_then(|notice| notice.action_id)
        .expect("Copy anyway");
    app.act(UiAction::Menu { id: anyway });
    app.wait_until("the snapshot's job ended", || !app.core().tools.tool().busy());
    app.settle();
    lines.push(format!("its Copy anyway | {}", a_box(&app, &restricted, 0, true)));

    open_locked(&app, &locked);
    tool_on(&app);
    take_box(&app);
    let said = notice(&app);
    let (offer, bytes) = offered(&app).expect("offered");
    let given = if bytes.iter().all(|(_, made)| made.as_ref().is_some_and(|made| !made.is_empty())) {
        "every type given"
    } else {
        "a type not given"
    };
    let text = bytes
        .iter()
        .find(|(uti, _)| uti == extract::UTI_TEXT)
        .and_then(|(_, made)| made.clone())
        .map(|made| String::from_utf8_lossy(&made).into_owned())
        .unwrap_or_default();
    lines.push(format!("opened with its password | {said} | {} | {given} | text {text:?}", types(&offer)));
    open_locked(&app, &locked_restricted);
    tool_on(&app);
    take_box(&app);
    lines.push(format!(
        "opened with its password, asking not to be copied | {} | {}",
        notice(&app),
        offered_or_nothing(&app)
    ));

    assert_eq!(lines, GOLDEN, "\nnow:\n{}", lines.join("\n"));
}

fn offered_or_nothing(app: &HeadlessApp) -> String {
    match offered(app) {
        Some((offer, _)) => types(&offer),
        None => "nothing offered".into(),
    }
}

/// Opens another file, and turns the tool on there, as a tab come forward turns it off.
fn open_with_tool(app: &HeadlessApp, path: &Path) {
    app.open(path);
    app.settle();
    tool_on(app);
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
    assert!(app.reader().password_prompt.is_none(), "it opened");
}

/// The session held by a thread of the test's until what's returned is dropped, so a command that reads it waits.
fn session_held(app: &HeadlessApp) -> (std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    let core = std::sync::Arc::clone(app.core());
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let holder = std::thread::Builder::new()
        .name("test-session-holder".into())
        .spawn(move || {
            core.host.session.with_session(|_| {
                held_tx.send(()).expect("the test is waiting");
                let _ = done_rx.recv_timeout(WAIT_LIMIT);
            });
        })
        .expect("a thread");
    held_rx.recv_timeout(WAIT_LIMIT).expect("the session held");
    (done_tx, holder)
}

/// **Failure (coordinator, 2026-11-14, the one place the UI thread waits): the system's paste of a promised SVG waits without end** for the person's
/// command making it, or waits on the session's lock itself, so the window stops. With the session held, the paste's command can't find its page: the
/// paste, on a thread standing in for the UI thread, gives up when the app's clock passes [`snapshot_tool::PASTE_PATIENCE_MS`], though the session is
/// still held, so it took no lock the session's holder has; the command, let go, makes nothing for it; and a paste after gets its SVG.
#[test]
fn a_paste_waits_at_most_its_patience_and_never_for_the_session() {
    let disk = Disk::new();
    let doc = disk.file("snapshot.pdf", &text_and_image());
    let app = disk.start();
    app.open(&doc);
    app.settle();
    tool_on(&app);
    take_box(&app);
    let inner = app.core();
    let offer = inner.tools.tool().take_offer(&inner.panes.engines(&inner.engine)).expect("offered");
    let (release, holder) = session_held(&app);
    let core = std::sync::Arc::clone(app.core());
    let serial = offer.serial;
    let ui = std::thread::Builder::new()
        .name("test-ui".into())
        .spawn(move || {
            let tool = core.tools.tool();
            core.act(UiAction::Pasted {
                serial,
                uti: extract::UTI_SVG.into(),
            })
            .understood
            .then(|| tool.wait_for_paste())
            .flatten()
        })
        .expect("a thread");
    app.wait_until("the paste's command is under way", || app.core().tools.tool().paste_running());
    app.advance(snapshot_tool::PASTE_PATIENCE_MS);
    app.wait_until("the paste gave up, the session still held", || ui.is_finished());
    assert_eq!(ui.join().expect("the paste's thread"), None, "nothing given past its patience");
    drop(release);
    holder.join().expect("the session's holder");
    app.wait_until("the paste's command ended", || !app.core().tools.tool().paste_running());
    let svg = clipboard_data(&app, serial, extract::UTI_SVG).expect("a paste after gets its SVG");
    assert!(String::from_utf8_lossy(&svg).contains("<svg"), "an SVG");
}

/// **Failure (S2's follow-ups, coordinator's Q6, 2026-11-14): a snapshot reads its page when its command starts,** so a ⌘R pressed right after the
/// button came up is in what's copied, or one pressed before it isn't. The snapshot holds the person's turn until its page is found: a turn asked
/// meanwhile waits, and is done after; what's copied is the page as it was when the box was let go.
#[test]
fn a_turn_asked_after_a_snapshot_isnt_copied() {
    let disk = Disk::new();
    let doc = disk.file("snapshot.pdf", &text_and_image());
    let app = disk.start();
    app.open(&doc);
    app.settle();
    tool_on(&app);
    let (held, go) = app.hold_next("snapshot");
    let_go_of_a_box(&app);
    held.recv_timeout(WAIT_LIMIT).expect("the snapshot held before it finds its page");
    app.act(UiAction::Menu { id: "rotate.right".into() });
    assert!(app.core().core.flush(WAIT_LIMIT), "the session thread took the turn");
    drop(go);
    app.wait_until("the snapshot's answered", || !app.core().tools.tool().busy());
    app.settle();
    assert_eq!(
        a_box(&app, &doc, 0, false),
        "\"Copied the snapshot as PNG, text, SVG and JPG.\" [] | [public.png, public.utf8-plain-text, public.svg-image*, public.jpeg] | same",
        "copied as the page was when the box was let go"
    );
    assert!(app.core().menu_item_enabled("edit.undo".into()), "and the turn done after it");
}

/// **Failure (coordinator, 2026-11-14: the clipboard's types are one copy of one moment): a page moved between the box and the paste of its SVG** gives
/// another page's SVG, as the paste names the page's place in the tab. The box on page 1 of two, the page moved after page 2, then the paste: no SVG, and
/// the PNG on the clipboard still the box's.
#[test]
fn a_page_moved_before_the_paste_gives_no_other_pages_svg() {
    let disk = Disk::new();
    let doc = disk.pdf("two.pdf", 2);
    let app = disk.start();
    app.open(&doc);
    app.settle();
    tool_on(&app);
    take_box(&app);
    let inner = app.core();
    let offer = inner.tools.tool().take_offer(&inner.panes.engines(&inner.engine)).expect("offered");
    app.agent(Level::Control, r#"{"cmd":"page_move","tab":"active","pages":"1","to":2}"#)
        .expect("moved");
    app.settle();
    let png = clipboard_data(&app, offer.serial, extract::UTI_PNG);
    let svg = clipboard_data(&app, offer.serial, extract::UTI_SVG);
    let box_png = reference(&doc, 0, false)
        .into_iter()
        .find(|(uti, _)| uti == extract::UTI_PNG)
        .map(|(_, bytes)| bytes);
    assert_eq!(svg, None, "no SVG of the page that's in its place now");
    assert!(png.is_some() && png == box_png, "the PNG is still the box's");
}

/// The session thread through what's queued now: what wasn't parked has run.
fn flush(app: &HeadlessApp) {
    assert!(app.core().core.flush(WAIT_LIMIT), "the session thread took what was queued");
}

/// **Failure (the branch's review, S1): a box or a click whose page moved under it copies another page.** The box and the click wait their turn in the
/// person's lane, and named their page by its place in the tab, so behind a highlight still landing and a page delete parked after it, the box drawn on
/// page 1 as the view still showed it was taken of whatever page was in its place once the delete had run: another page's picture and text. Each,
/// box then click, is refused in the tool's words for a page that can't be read now, and nothing is offered.
#[test]
fn a_box_or_click_whose_page_moved_under_it_copies_nothing() {
    let disk = Disk::new();
    let doc = disk.pdf("three.pdf", 3);
    let app = disk.start();
    app.open(&doc);
    app.settle();
    tool_on(&app);
    for (what, release) in [
        ("a box", (BOX[0], BOX[1], Some((BOX[0] + BOX[2], BOX[1] + BOX[3])))),
        ("a click", (120.0, 50.0, None)),
    ] {
        app.core().panes.reading_engine(&app.core().engine).selection().restore((0, 0), (0, 5));
        let (held, go) = app.hold_next("highlight");
        app.act(UiAction::Menu {
            id: crate::core::highlight::HIGHLIGHT_ID.into(),
        });
        held.recv_timeout(WAIT_LIMIT)
            .expect("the highlight held as its pages are read, holding its turn");
        app.person(r#"{"cmd":"page_delete","tab":"active","pages":"1"}"#);
        flush(&app);
        let (x, y, to) = release;
        let_go(&app, (x, y), to);
        flush(&app);
        drop(go);
        app.wait_until("the snapshot's answered", || !app.core().tools.tool().busy());
        app.settle();
        assert_eq!(
            (notice(&app), offered_or_nothing(&app)),
            (
                "\"That page can't be read now: its file has changed or gone.\" []".to_string(),
                "nothing offered".to_string()
            ),
            "{what} on the page the delete took away"
        );
    }
    let pages = app.core().panes.reading_engine(&app.core().engine).points().page_count();
    assert_eq!(pages, 1, "both deletes were done");
}

/// **Failure (the branch's review): a snapshot holds the person's turn while it draws,** so a ⌘Z asked while a heavy box is still being drawn waits for
/// the drawing, seconds on a map. Its turn passes once its page is found: with the snapshot held at its drawing, the ⌘Z is done.
#[test]
fn a_snapshot_still_drawing_doesnt_hold_an_undo_behind_it() {
    let disk = Disk::new();
    let doc = disk.pdf("one.pdf", 1);
    let app = disk.start();
    app.open(&doc);
    app.act(UiAction::Menu { id: "rotate.right".into() });
    app.settle();
    tool_on(&app);
    let (held, go) = app.hold_next("snapshot_drawing");
    let_go_of_a_box(&app);
    held.recv_timeout(WAIT_LIMIT).expect("the snapshot held as it draws");
    app.act(UiAction::Menu { id: "edit.undo".into() });
    flush(&app);
    assert!(
        app.core().menu_item_enabled("edit.redo".into()),
        "the turn undone while the snapshot still draws"
    );
    drop(go);
    app.wait_until("the snapshot's answered", || !app.core().tools.tool().busy());
    app.settle();
}
