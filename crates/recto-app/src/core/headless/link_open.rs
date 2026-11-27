//! **A link opened from outside, through the app core, with no window** (PLAN 8.4): a `rectopdf:` link the system hands over, a file the Finder hands over
//! with a `#page=N`, and an agent's `recto open --link`. Each is the one `open {path, page}`, run by the one executor; what only the real app does (Launch
//! Services delivering a click as an Apple event) can't be sent to a quiet run, so `UiAction::OpenLink` with the raw string stands for it.

use super::changed_under_edits::pdf_bytes;
use super::copy_link::pasteboard;
use crate::core::controls::UiAction;
use crate::testkit::headless::HeadlessApp;
use crate::{RowKind, SidebarRow};
use recto_agent::Level;
use std::path::{Path, PathBuf};

fn link_to(path: &Path, page: u32) -> String {
    recto_core::locator::address_of(path, page).expect("a link")
}

fn open_link(app: &HeadlessApp, link: &str) {
    app.act(UiAction::OpenLink { link: link.into() });
    app.settle();
}

/// The active tab's page, counted from 1, as `where` says it.
fn page(app: &HeadlessApp) -> u64 {
    app.recto(Level::ReadOnly, &["where"]).expect("where")["page"].as_u64().expect("a page")
}

fn tab_count(app: &HeadlessApp) -> usize {
    app.tabs().iter().filter(|row| row.kind == RowKind::Tab).count()
}

fn goto(app: &HeadlessApp, page: u32) {
    app.recto(Level::Control, &["goto", &page.to_string()]).expect("goto");
    app.settle();
}

fn failure(app: &HeadlessApp) -> Option<String> {
    app.core().window.notices().open_failure()
}

fn with_pages(name: &str, pages: usize) -> (HeadlessApp, PathBuf) {
    let app = HeadlessApp::start();
    let file = app.disk().file(name, &pdf_bytes("Mine", pages));
    app.open(&file);
    (app, file)
}

/// Failure: a link copied from a tab with unsaved moves is opened at the number of the page in the file, not the tab: the click lands on another page than the
/// one that was copied. After page 1 is moved to 3, Copy Link of the tab's page 3 is `#page=1`; the click (the system's open, from the clipboard text) and an
/// agent's `open --link` of the same text both land on the tab's page 3 again, from wherever the tab was.
#[test]
fn copy_link_then_a_click_lands_on_the_page_copied() {
    let (app, _file) = with_pages("moved.pdf", 5);
    app.person(r##"{"cmd":"page_move","tab":"#1","pages":"1","to":3}"##);
    app.settle();
    let row = SidebarRow {
        kind: RowKind::Contents,
        page: Some(3),
        ..app.tabs()[0].clone()
    };
    app.act(UiAction::CopyLink { row });
    app.settle();
    let text = pasteboard(&app).expect("a copy");
    assert!(text.starts_with("rectopdf:///") && text.ends_with("#page=1"), "{text}");
    goto(&app, 5);
    open_link(&app, &text);
    assert_eq!((page(&app), tab_count(&app)), (3, 1), "the click lands on the page that was copied");
    goto(&app, 5);
    let said = app.recto(Level::Control, &["open", "--link", &text]).expect("an agent's open");
    assert_eq!((said["page"].as_u64(), said["file_page"].as_u64()), (Some(3), Some(1)), "{said}");
    assert_eq!(page(&app), 3);
}

/// Failure: the Finder's fragment, a click on either form of a link and an agent's link are read by different code, so one lands on another page than the
/// others. The same file and page by each route reaches the same page, through the one executor.
#[test]
fn the_finders_fragment_a_click_and_an_agents_link_land_alike() {
    let (app, file) = with_pages("alike.pdf", 6);
    let path = file.display().to_string();
    app.act(UiAction::OpenAt {
        path: path.clone(),
        fragment: "page=3".into(),
    });
    app.settle();
    assert_eq!(page(&app), 3, "the Finder's fragment");
    let link = link_to(&file, 4);
    goto(&app, 1);
    open_link(&app, &link.replacen("rectopdf:", "file:", 1));
    assert_eq!(page(&app), 4, "a file address");
    goto(&app, 1);
    open_link(&app, &link);
    assert_eq!(page(&app), 4, "a rectopdf link");
    goto(&app, 1);
    app.recto(Level::Control, &["open", "--link", &link]).expect("an agent's open");
    assert_eq!(page(&app), 4, "an agent's link");
    app.act(UiAction::OpenAt {
        path,
        fragment: "zoom=200&nameddest=x".into(),
    });
    app.settle();
    assert_eq!(
        (page(&app), tab_count(&app)),
        (4, 1),
        "a fragment with no page moves nothing and opens no second tab"
    );
}

/// Failure: a link to an open file opens it again in a second tab, or goes to its page without bringing its tab forward. Open A, open B, then a link to
/// A's page 2: still two tabs, A in front, at page 2.
#[test]
fn a_link_to_an_open_file_comes_to_its_tab() {
    let app = HeadlessApp::start();
    let a = app.disk().file("a.pdf", &pdf_bytes("A", 3));
    let b = app.disk().file("b.pdf", &pdf_bytes("B", 3));
    app.open(&a);
    app.open(&b);
    open_link(&app, &link_to(&a, 2));
    assert_eq!(tab_count(&app), 2);
    let now = app.recto(Level::ReadOnly, &["where"]).expect("where");
    assert_eq!((now["tab"].as_u64(), now["page"].as_u64()), (Some(1), Some(2)), "{now}");
}

/// Failure: a bad link opens something (another file, a tab) or says nothing, or its words hold part of the link, so a page can put its own text in front of
/// the person. Each is refused in Rust's own words, which hold none of what was sent, and opens no tab.
#[test]
fn a_bad_link_says_why_and_opens_nothing() {
    let (app, file) = with_pages("only.pdf", 2);
    app.act(UiAction::Menu { id: "file.close".into() });
    app.settle();
    let opened = tab_count(&app);
    let missing = app.disk().files().join("MARKERgone.pdf");
    let big = format!("rectopdf:///{}", "MARKER".repeat(1 << 18));
    let bad = [
        "http://MARKER/a.pdf".to_string(),
        "rectopdf://MARKERhost/a.pdf".to_string(),
        "rectopdf:///a/%2E%2E/MARKERb.pdf".to_string(),
        big,
        format!("{}#page=0", link_to(&file, 1).split('#').next().unwrap()),
        link_to(&file, 99),
        link_to(&missing, 1),
    ];
    for link in &bad {
        app.core().window.notices().clear_open_failure();
        open_link(&app, link);
        let said = failure(&app).unwrap_or_else(|| panic!("nothing said for {:?}", &link[..link.len().min(60)]));
        assert!(!said.contains("MARKER"), "{said}");
        assert_eq!(tab_count(&app), opened, "no new tab for {:?}", &link[..link.len().min(60)]);
    }
}

/// Failure: a page the unsaved edits removed is guessed at (the nearest, or the same number), or a renumbered page isn't said. With page 2 deleted, a link
/// to file page 2 leaves the tab where it was and says so; one to file page 3 lands on the tab's page 2 and says the renumbering.
#[test]
fn a_page_removed_by_unsaved_edits_is_said_not_guessed() {
    let (app, file) = with_pages("edited.pdf", 4);
    app.person(r##"{"cmd":"page_delete","tab":"#1","pages":"2"}"##);
    app.settle();
    goto(&app, 3);
    app.core().window.notices().take_notice();
    open_link(&app, &link_to(&file, 2));
    assert_eq!(page(&app), 3, "the tab stays where it was");
    let said = app.core().window.notices().notice().expect("it says so");
    assert!(said.contains("isn't in this tab now"), "{said}");
    open_link(&app, &link_to(&file, 3));
    assert_eq!(page(&app), 2);
    let said = app.core().window.notices().notice().expect("it says the renumbering");
    assert!(said.contains("Page 3 of the saved file is page 2 here"), "{said}");
}

/// Failure: a page or a flood of links from outside is taken without bound (a page that loops can open files without end), or the person is told over and
/// over, or the cap never lifts. Nine links in ten seconds open eight and say so once; after the span one more opens.
#[test]
fn links_from_outside_past_the_cap_open_nothing_and_say_so_once() {
    let app = HeadlessApp::start();
    let files: Vec<PathBuf> = (0..10).map(|n| app.disk().file(&format!("f{n}.pdf"), &pdf_bytes("F", 2))).collect();
    // One at a time, as they arrive: an open asked for again before the last one's read lands takes its place (the last thing asked for wins).
    for file in &files[..9] {
        open_link(&app, &link_to(file, 1));
    }
    assert_eq!(tab_count(&app), 8, "the ninth wasn't opened");
    let said = failure(&app).expect("the person is told");
    assert!(said.contains("More links arrived at once"), "{said}");
    app.core().window.notices().clear_open_failure();
    open_link(&app, &link_to(&files[9], 1));
    assert_eq!((failure(&app), tab_count(&app)), (None, 8), "told once, and the rest still not opened");
    app.advance(10_001);
    open_link(&app, &link_to(&files[8], 1));
    assert_eq!(tab_count(&app), 9, "after the span it opens again");
}

/// Failure: a link that arrives while the app is handing off to an update or quitting opens a tab after all (queued behind the freeze, or when it ends).
#[test]
fn a_link_during_a_quit_or_an_update_opens_nothing() {
    let app = HeadlessApp::start();
    let file = app.disk().file("frozen.pdf", &pdf_bytes("F", 2));
    app.core().set_frozen(true);
    open_link(&app, &link_to(&file, 1));
    assert_eq!(tab_count(&app), 0, "during the freeze");
    app.core().set_frozen(false);
    app.settle();
    assert_eq!(tab_count(&app), 0, "and not when it ends");
}

/// Failure: a link's open keeps the link. The link's text (an ignored `zzmarker` in its fragment, which a page chose) is in no notice, published state or
/// record.
#[test]
fn an_outside_links_open_is_the_persons_and_keeps_no_link() {
    let (app, file) = with_pages("kept.pdf", 3);
    app.act(UiAction::Menu { id: "file.close".into() });
    app.settle();
    open_link(&app, &format!("{}&zzmarker=1", link_to(&file, 2)));
    assert_eq!(page(&app), 2);
    // The command run is `open {path, page}`, the person's: `coverage.rs`'s golden holds what each of these controls sends. The headless app writes no
    // activity log (see `password_not_kept.rs`); what a command's log line holds is `recto-agent`'s `a_links_wire_holds_its_path_and_page_and_no_link`.
    for held in [
        app.session_json().to_string(),
        app.record_json().to_string(),
        format!("{:?}", app.core().window.notices().notice()),
        format!("{:?}", failure(&app)),
    ] {
        assert!(!held.contains("zzmarker"), "the link was kept: {held}");
    }
}

/// Failure: an agent below Control opens a file through a link, which no `open` allows it.
#[test]
fn an_agent_below_control_opens_no_link() {
    let app = HeadlessApp::start();
    let file = app.disk().file("read.pdf", &pdf_bytes("R", 2));
    let link = link_to(&file, 1);
    let error = app.recto(Level::ReadOnly, &["open", "--link", &link]).expect_err("refused at Read");
    assert_eq!(error.code, recto_agent::Code::PERMISSION_DENIED);
    assert_eq!(tab_count(&app), 0);
}
