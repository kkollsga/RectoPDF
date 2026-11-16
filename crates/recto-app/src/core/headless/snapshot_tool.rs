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
    // What the window was last given is the answer's notice, not "Copying…": the coverage golden holds the host's calls by kind, as both orders of the
    // clipboard's and the window's are right, so it's held here (the phase's closing read, 2026-11-15).
    let shown = app.reader().selection_notice.map(|notice| notice.text);
    assert_eq!(
        shown.as_deref(),
        Some("Copied the snapshot as PNG, text, SVG and JPG."),
        "the window shows the answer"
    );
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

/// **Failure (the branch's review, F3): a snapshot whose thread panics leaves the tool dead,** as its answer never came: the tool busy for good, a hand-off
/// and every settle waiting on it, and no snapshot taken again. A panic seeded as it draws: the person is told it wasn't taken, the tool's at rest, and
/// the next box copies as the golden has it.
#[test]
fn a_snapshot_that_panics_leaves_the_tool_working() {
    let disk = Disk::new();
    let doc = disk.file("snapshot.pdf", &text_and_image());
    let app = disk.start();
    app.open(&doc);
    app.settle();
    tool_on(&app);
    app.panic_next("snapshot_drawing");
    take_box(&app);
    assert_eq!(notice(&app), "\"The snapshot couldn't be taken.\" []", "the person is told");
    assert_eq!(offered_or_nothing(&app), "nothing offered");
    take_box(&app);
    assert_eq!(
        a_box(&app, &doc, 0, false),
        "\"Copied the snapshot as PNG, text, SVG and JPG.\" [] | [public.png, public.utf8-plain-text, public.svg-image*, public.jpeg] | same",
        "the next box copies"
    );
}

/// **Failure (the branch's review, F3): a paste whose thread panics leaves every later paste of an SVG giving nothing,** as the paste was counted under
/// way for good. A panic seeded as its SVG is drawn: that paste gets nothing, and the next gets its SVG.
#[test]
fn a_paste_that_panics_leaves_the_next_paste_working() {
    let disk = Disk::new();
    let doc = disk.file("snapshot.pdf", &text_and_image());
    let app = disk.start();
    app.open(&doc);
    app.settle();
    tool_on(&app);
    take_box(&app);
    let inner = app.core();
    let tool = inner.tools.tool();
    let offer = tool.take_offer(&inner.panes.engines(&inner.engine)).expect("offered");
    app.panic_next("paste_drawing");
    let pasted = app.act(UiAction::Pasted {
        serial: offer.serial,
        uti: extract::UTI_SVG.into(),
    });
    assert!(pasted.understood, "the paste's command sent");
    app.wait_until("the paste that panicked ended", || !tool.paste_running());
    assert_eq!(tool.wait_for_paste(), None, "it gives nothing");
    let svg = clipboard_data(&app, offer.serial, extract::UTI_SVG).expect("the next paste gets its SVG");
    assert!(String::from_utf8_lossy(&svg).contains("<svg"), "an SVG");
}

/// **Failure (the phase's closing read, 2026-11-15): a box parked behind the person's highlight and then dropped from the lane was never answered,** so the
/// tool said "Copying…" for good and was dead: busy, a hand-off and every settle waiting on it, and no box taken again. Dropped at the cap, and with what
/// was parked at a quit's patience the person then stayed: the page says the lane's words once, the tool is at rest, and the next box copies as the
/// golden has it.
#[cfg(unix)]
#[test]
fn a_box_dropped_from_the_lane_leaves_the_tool_working() {
    use super::persons_lane::{Dropped, parked_then_dropped};
    for how in [Dropped::AtTheCap, Dropped::AtAQuit] {
        let disk = Disk::new();
        let doc = disk.file("snapshot.pdf", &text_and_image());
        let app = disk.start();
        app.open(&doc);
        app.settle();
        tool_on(&app);
        parked_then_dropped(&app, how, || let_go_of_a_box(&app));
        let shown = app.reader().selection_notice.map(|notice| notice.text);
        assert_eq!(
            (shown.as_deref(), notice(&app), app.core().tools.tool().busy()),
            (Some(how.words()), "-".to_string(), false),
            "{how:?}: said once, by the page, and the tool at rest"
        );
        app.core().dismiss_selection_notice();
        take_box(&app);
        // The quit's case turned the page first, unsaved, so that the quit asks.
        let turns = u8::from(matches!(how, Dropped::AtAQuit));
        assert_eq!(
            a_box(&app, &doc, turns, false),
            "\"Copied the snapshot as PNG, text, SVG and JPG.\" [] | [public.png, public.utf8-plain-text, public.svg-image*, public.jpeg] | same",
            "{how:?}: the next box copies"
        );
    }
}

/// **Failure (found on the way, 2026-11-15): a quit the person stayed after left the snapshot tool quitting for good,** so every later paste of a box's
/// promised SVG got nothing until the app was started again. With a page turned unsaved and the record unwritable, so the quit asks, and Stay: a box
/// taken after gives its SVG.
#[cfg(unix)]
#[test]
fn a_paste_after_a_stayed_quit_gets_its_svg() {
    use super::persons_lane::{quit_answers, record_folder_writable};
    let disk = Disk::new();
    let doc = disk.file("snapshot.pdf", &text_and_image());
    let app = disk.start();
    app.open(&doc);
    app.act(UiAction::Menu { id: "rotate.right".into() });
    app.settle();
    tool_on(&app);
    record_folder_writable(&app, false);
    app.core().quit_requested();
    app.wait_until("the quit's question", || quit_answers(&app) == 1);
    app.stay();
    record_folder_writable(&app, true);
    app.settle();
    take_box(&app);
    let (_, bytes) = offered(&app).expect("offered");
    let svg = bytes.into_iter().find(|(uti, _)| uti == extract::UTI_SVG).and_then(|(_, svg)| svg);
    assert!(svg.is_some_and(|svg| String::from_utf8_lossy(&svg).contains("<svg")), "the paste gets its SVG");
}

/// **Failure (the phase's closing read, 2026-11-15): a paste of a promised SVG held while a late keep's saved session was read, then not taken past a
/// freeze as the restore let it go, was never answered,** so the paste was counted under way for good and every later paste got nothing. Held, frozen,
/// the restore let in: the paste's no longer under way and gives nothing; unfrozen, a paste of a box taken on the restored session gets its SVG.
#[test]
fn a_paste_held_for_a_restore_and_dropped_by_a_freeze_leaves_the_next_paste_working() {
    use super::late_lock::{a_saved_session, let_the_restore_in, restoring_held};
    let app = restoring_held(a_saved_session());
    let tool = app.core().tools.tool();
    let serial = tool.offer_a_promise_for_a_test(snapshot_tool::Promise {
        tab: 1,
        position: 0,
        rect: BOX,
        as_seen: extract::AsSeen {
            quarter_turns: 0,
            file_page: 0,
            own: true,
        },
    });
    let pasted = app.act(UiAction::Pasted {
        serial,
        uti: extract::UTI_SVG.into(),
    });
    assert!(pasted.understood && tool.paste_running(), "the paste's command sent, and held for the restore");
    app.core().set_frozen(true);
    let_the_restore_in(&app);
    assert!(!tool.paste_running(), "answered, though it was never run");
    assert_eq!(tool.wait_for_paste(), None, "and it gives nothing");
    app.core().set_frozen(false);
    tool_on(&app);
    take_box(&app);
    let (_, bytes) = offered(&app).expect("offered");
    let svg = bytes.into_iter().find(|(uti, _)| uti == extract::UTI_SVG).and_then(|(_, svg)| svg);
    assert!(
        svg.is_some_and(|svg| String::from_utf8_lossy(&svg).contains("<svg")),
        "the next paste gets its SVG"
    );
    drop(app.quit());
}

/// **Failure (a guard put in before Capture, 2026-11-16): the tool turned on from the Tools menu stops staying on,** so a person taking several boxes in a
/// row has to turn it on again for each. A box, a click on an image and a click on nothing in turn, each taken, and the tool is still on and drawing
/// nothing after every one.
#[test]
fn the_tool_turned_on_from_the_menu_stays_on_after_each_capture() {
    let disk = Disk::new();
    let doc = disk.file("snapshot.pdf", &text_and_image());
    let app = disk.start();
    app.open(&doc);
    app.settle();
    tool_on(&app);
    for (what, from, to) in [
        ("a box", (BOX[0], BOX[1]), Some((BOX[0] + BOX[2], BOX[1] + BOX[3]))),
        ("a click on an image", (120.0, 270.0), None),
        ("a click on nothing", (400.0, 600.0), None),
    ] {
        mouse(&app, from, to);
        let tool = app.core().tools.tool();
        assert!(tool.is_on() && !tool.is_drawing(), "{what}: the tool is still on, and the box gone");
    }
}

/// Chooses Capture from the page's right-click menu, as the window does: the one id, to the one executor.
fn capture(app: &HeadlessApp) {
    app.core().perform(snapshot_tool::CAPTURE_ID.into());
    app.settle();
}

/// The tool as `qa state` reports it: on, and whether a box is being drawn.
fn on_and_drawing(app: &HeadlessApp) -> (bool, bool) {
    let tool = app.core().tools.tool();
    (tool.is_on(), tool.is_drawing())
}

/// **Failure (owner, 2026-11-16: "Capture ... will trigger a single capture, reverting to normal mode after"): the tool stays on after a Capture's release,**
/// so the next drag draws a box instead of selecting text; or it goes off without taking what the release took; or a second capture needs the tool turned on
/// again though Capture was chosen. A box, a click on an image and a click on nothing each: Capture turns the tool on (the menu's tick, the crosshair and
/// `qa state` all read this flag), one release takes what it takes as the person's snapshot and turns it off, and the next press is text selection's.
#[test]
fn capture_takes_one_release_and_the_tool_goes_off() {
    let disk = Disk::new();
    let doc = disk.file("snapshot.pdf", &text_and_image());
    let app = disk.start();
    app.open(&doc);
    app.settle();
    for (what, from, to, said) in [
        (
            "a box",
            (BOX[0], BOX[1]),
            Some((BOX[0] + BOX[2], BOX[1] + BOX[3])),
            "\"Copied the snapshot as PNG, text, SVG and JPG.\" []",
        ),
        (
            "a click on an image",
            (120.0, 270.0),
            None,
            "\"Copied the image as its own JPEG, 4 × 4 pixels.\" []",
        ),
        (
            "a click on nothing",
            (400.0, 600.0),
            None,
            "\"There's no image there. Drag a box to take part of the page.\" []",
        ),
    ] {
        assert!(!app.core().tools.tool().is_on(), "{what}: text selection to begin with");
        capture(&app);
        assert_eq!(on_and_drawing(&app), (true, false), "{what}: Capture turns the tool on");
        assert!(
            app.core()
                .bar_now()
                .iter()
                .any(|menu| menu.entries.iter().any(|item| item.id == snapshot_tool::TOGGLE_ID && item.checked)),
            "{what}: the Tools menu ticks it"
        );
        mouse(&app, from, to);
        assert_eq!(on_and_drawing(&app), (false, false), "{what}: one release and it's off");
        assert_eq!(notice(&app), said, "{what}: taken as the tool takes it");
        assert!(
            !app.core()
                .bar_now()
                .iter()
                .any(|menu| menu.entries.iter().any(|item| item.id == snapshot_tool::TOGGLE_ID && item.checked)),
            "{what}: unticked"
        );
        let inner = app.core();
        let engine = inner.panes.reading_engine(&inner.engine);
        let (x, y) = at(&app, 50.0, 40.0);
        assert!(!inner.tools.tool().is_on());
        drop(engine.selection().press(x, y, 1, false));
        assert!(!inner.tools.tool().is_drawing(), "{what}: the next press is text selection's");
    }
}

/// **Failure (owner, 2026-11-16): a capture refused for a copy-restricted file loses its "Copy anyway" because the tool has gone off,** or the refusal
/// is dropped with the tool. Capture, a box on a file that asks not to be copied: the notice offers Copy anyway with the tool off, and it copies.
#[test]
fn copy_anyway_still_works_after_a_refused_capture() {
    let disk = Disk::new();
    let restricted = disk.file("restricted.pdf", &locked_allowing("", 1, lopdf::encryption::Permissions::PRINTABLE));
    let app = disk.start();
    app.open(&restricted);
    app.settle();
    capture(&app);
    take_box(&app);
    assert!(!app.core().tools.tool().is_on(), "the refused capture was the one");
    assert_eq!(notice(&app), "\"This file's permissions ask that it isn't copied.\" [Copy anyway]");
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
    assert_eq!(notice(&app), "\"Copied the snapshot as PNG, text, SVG and JPG.\" []");
    assert!(!app.core().tools.tool().is_on(), "and still off");
}

/// **Failure (owner, 2026-11-16): Escape, or the tool's own toggle, doesn't end a Capture that wasn't taken,** leaving the tool on with no way back but
/// taking something; or Escape turns off the tool the person turned on from the menu bar, which isn't a capture and stays until it's turned off; or a
/// Capture chosen with the tool already on from the menu bar leaves it on after the release. Nothing is taken in any case but the last.
#[test]
fn escape_and_the_toggle_end_a_capture_and_the_menu_bars_tool_is_left_alone() {
    let disk = Disk::new();
    let doc = disk.file("snapshot.pdf", &text_and_image());
    let app = disk.start();
    app.open(&doc);
    app.settle();
    capture(&app);
    app.core().perform(snapshot_tool::CANCEL_ID.into());
    assert_eq!(on_and_drawing(&app), (false, false), "Escape ends it");
    capture(&app);
    app.core().perform(snapshot_tool::TOGGLE_ID.into());
    assert!(!app.core().tools.tool().is_on(), "the toggle ends it, as it ends any tool that's on");
    assert!(!app.core().tools.tool().is_once());
    assert!(
        app.core().tools.tool().selection_notice().is_none() && app.core().tools.tool().notice().is_none(),
        "nothing was taken"
    );
    tool_on(&app);
    app.core().perform(snapshot_tool::CANCEL_ID.into());
    assert!(app.core().tools.tool().is_on(), "Escape leaves the menu bar's tool on");
    capture(&app);
    assert!(
        app.core().tools.tool().is_on() && app.core().tools.tool().is_once(),
        "Capture with the tool on leaves it on, one-shot"
    );
    take_box(&app);
    assert!(!app.core().tools.tool().is_on(), "and the release ended it");
}

/// **Failure (owner, 2026-11-16): Escape is taken by the window with no capture to end, or isn't sent with one,** so it stops doing what it does elsewhere,
/// or can't end a capture. The key's id is Capture's end while one is armed, and nothing otherwise.
#[test]
fn escape_is_the_windows_only_while_a_capture_waits() {
    assert_eq!(crate::snapshot_menu::plain_key("\u{1b}", true), Some(snapshot_tool::CANCEL_ID));
    assert_eq!(crate::snapshot_menu::plain_key("\u{1b}", false), None);
    assert_eq!(crate::snapshot_menu::plain_key("a", true), None);
}

/// **Failure (AGENTS.md: "An announcement is made when something changed, and only then"): the host is told again for a Capture that changed nothing, or an
/// Escape with no capture to end, or never told the tool went off after the release,** so the Tools menu's tick and the crosshair stay as they were. On and
/// off once each, and nothing for what changed nothing.
#[test]
fn the_host_is_told_when_a_capture_begins_and_ends_and_only_then() {
    use crate::testkit::Heard;
    let disk = Disk::new();
    let doc = disk.file("snapshot.pdf", &text_and_image());
    let app = disk.start();
    app.open(&doc);
    app.settle();
    let told = |app: &HeadlessApp| app.listener().count(&Heard::ModelChanged);
    let before = told(&app);
    capture(&app);
    assert_eq!(told(&app) - before, 1, "told it's on");
    capture(&app);
    assert_eq!(told(&app) - before, 1, "Capture again changed nothing");
    app.core().perform(snapshot_tool::CANCEL_ID.into());
    app.settle();
    assert_eq!(told(&app) - before, 2, "told it's off");
    app.core().perform(snapshot_tool::CANCEL_ID.into());
    app.settle();
    assert_eq!(told(&app) - before, 2, "Escape with nothing to end changed nothing");
    capture(&app);
    let armed = told(&app);
    // A box too small to take anything is a click that takes the image under it: any release is told.
    let_go(&app, (400.0, 600.0), None);
    assert!(told(&app) > armed, "the release that ended it told the host");
    app.wait_until("the job's answered", || !app.core().tools.tool().busy());
    app.settle();
}

/// **Failure (AGENTS.md: "Nothing an agent sends runs as the person"): Capture is run for an agent wider than the tool's own toggle,** at a level that can't
/// turn the tool on, or as the person (a snapshot to the person's clipboard from an agent's id). An agent's run of Capture does at each level exactly what
/// its run of `tools.snapshot` does.
#[test]
fn an_agents_capture_is_what_its_snapshot_toggle_is() {
    let disk = Disk::new();
    let doc = disk.file("snapshot.pdf", &text_and_image());
    let app = disk.start();
    app.open(&doc);
    app.settle();
    for level in [Level::ReadOnly, Level::Control, Level::Full] {
        let run = |id: &str| {
            let result = app
                .core()
                .host
                .run_menu_id(recto_agent::Actor::Agent(level), app.core(), id, None, &mut Vec::new());
            app.settle();
            let outcome = (result.map(drop).map_err(|error| error.code.to_string()), app.core().tools.tool().is_on());
            if app.core().tools.tool().is_on() {
                app.core().perform(snapshot_tool::TOGGLE_ID.into());
            }
            outcome
        };
        assert_eq!(run(snapshot_tool::CAPTURE_ID), run(snapshot_tool::TOGGLE_ID), "{level:?}");
    }
}
