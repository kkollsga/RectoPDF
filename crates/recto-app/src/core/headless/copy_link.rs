//! **Copy Link through the app core, with no window** (PLAN 8.4): Edit > Copy Link and a contents row's Copy Link send the person's `recto link <page>
//! --to-clipboard`; the address lands on the clipboard as plain text and nowhere else. What only the real app shows (the pasteboard itself) is the one
//! launch's, `qa copied`.

use super::changed_under_edits::pdf_bytes;
use crate::core::controls::UiAction;
use crate::testkit::Heard;
use crate::testkit::headless::HeadlessApp;
use crate::{RowKind, SidebarRow};
use recto_agent::{Actor, Level};
use serde_json::json;
use std::path::Path;

const ID: &str = crate::core::copy_link::COPY_LINK_ID;

fn menu() -> UiAction {
    UiAction::Menu { id: ID.into() }
}

/// What the host would put on the clipboard now, if it is plain text and the only type: the text.
fn pasteboard(app: &HeadlessApp) -> Option<String> {
    let inner = app.core();
    let offer = inner.tools.tool().take_offer(&inner.panes.engines(&inner.engine))?;
    assert_eq!(offer.types, ["public.utf8-plain-text"], "plain text only: no file URL, no URL type");
    let bytes = inner.tools.tool().ready_bytes(offer.serial, "public.utf8-plain-text").expect("its bytes");
    Some(String::from_utf8(bytes).expect("text"))
}

/// What a reader of the address does with its path: the bytes, undone from `%XX`.
fn path_of(address: &str) -> (String, &str) {
    let (path, page) = address.strip_prefix("file://").expect("a file address").split_once('#').expect("a page");
    let bytes = path.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            out.push(u8::from_str_radix(&path[at + 1..at + 3], 16).expect("hex"));
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    (String::from_utf8(out).expect("utf-8"), page)
}

fn selects_on_page(app: &HeadlessApp, page: u32) {
    app.core().panes.reading_engine(&app.core().engine).selection().restore((page, 0), (page, 5));
}

fn contents_row(app: &HeadlessApp, page: u32) -> SidebarRow {
    SidebarRow {
        kind: RowKind::Contents,
        page: Some(page),
        ..app.tabs()[0].clone()
    }
}

fn open_with(name: &str, pages: usize) -> (HeadlessApp, std::path::PathBuf) {
    let app = HeadlessApp::start();
    let file = app.disk().file(name, &pdf_bytes("Mine", pages));
    app.open(&file);
    (app, file)
}

/// Failure: Copy Link on a selection on page 2 puts another page, the folder-less name, an address with a raw `#` or `%` in it, or more than plain text on
/// the clipboard, or the host isn't told there's a copy. The address is the file's whole path and `#page=2`, and the host is told once.
#[test]
fn edit_copy_link_puts_the_files_address_and_page_on_the_clipboard_as_text() {
    let (app, file) = open_with("a b#1.pdf", 4);
    selects_on_page(&app, 1);
    app.act(menu());
    app.settle();
    let copies = app.heard().iter().filter(|heard| matches!(heard, Heard::CopyReady)).count();
    assert_eq!(copies, 1, "the host is told once");
    let address = pasteboard(&app).expect("a copy");
    assert_eq!(address.matches('#').count(), 1, "{address}");
    let (path, page) = path_of(&address);
    assert_eq!((Path::new(&path), page), (file.as_path(), "page=2"), "{address}");
    let said = app.core().window.notices().notice().expect("said");
    assert_eq!(
        said, "Copied a link to page 2. Preview may open it at the first page.",
        "the owner's decision of 2026-11-26: it says so"
    );
    // Nothing selected sends nothing.
    selects_on_page(&app, 0);
    app.core().panes.reading_engine(&app.core().engine).selection().clear_soon();
    app.settle();
    app.act(menu());
    app.settle();
    assert_eq!(pasteboard(&app), None, "no selection, no copy");
}

/// Failure: a contents row's Copy Link copies the tab's page and not the page a tap on that row goes to, or a row past the end of what the reader has
/// copies a page that isn't there, where a tap lands on the last.
#[test]
fn a_contents_rows_copy_link_is_the_page_a_tap_on_it_goes_to() {
    let (app, _file) = open_with("c.pdf", 4);
    app.act(UiAction::CopyLink { row: contents_row(&app, 3) });
    app.settle();
    assert!(pasteboard(&app).expect("a copy").ends_with("#page=3"));
    app.act(UiAction::CopyLink { row: contents_row(&app, 99) });
    app.settle();
    assert!(
        pasteboard(&app).expect("a copy").ends_with("#page=4"),
        "the last page the reader has, as a tap goes"
    );
}

/// Failure: an agent's palette run of Copy Link, at any level, is done as the person (the address on the clipboard) or refused without naming `recto link`,
/// the command it has for itself; or its refusal leaves something offered or sent.
#[test]
fn an_agents_run_of_copy_link_is_refused_and_offers_nothing() {
    let (app, _file) = open_with("d.pdf", 3);
    selects_on_page(&app, 0);
    app.keep_sent();
    for level in [Level::Control, Level::Full] {
        let core = app.core();
        let error = core
            .host
            .run_menu_id(Actor::Agent(level), core, ID, None, &mut Vec::new())
            .map(drop)
            .expect_err("refused");
        app.settle();
        assert_eq!(error.code.as_str(), "persons_own", "{level:?}");
        assert!(error.next.contains("`recto link`"), "{level:?}: {error:?}");
    }
    assert_eq!(pasteboard(&app), None, "nothing offered");
    assert!(app.sent().is_empty(), "nothing sent as the person: {:?}", app.sent());
}

/// Failure: the person copying the link of an added page is told in an agent's words (`not_referenceable`, "Save first, or ask for…"), or gets another
/// page's address, or the clipboard is changed. The person's own words, and nothing offered.
#[test]
fn a_page_added_from_another_pdf_says_so_in_the_persons_words_and_copies_nothing() {
    let (app, _file) = open_with("e.pdf", 3);
    let other = app.disk().file("other.pdf", &pdf_bytes("Other", 1));
    app.person(&json!({ "cmd": "page_insert", "tab": "#1", "path": other, "at": 1 }).to_string());
    app.settle();
    app.act(UiAction::CopyLink { row: contents_row(&app, 1) });
    app.settle();
    assert_eq!(pasteboard(&app), None);
    let said = app.core().window.notices().notice().expect("said");
    assert!(
        said.contains("another PDF") && !said.contains("recto") && !said.contains("Save first, or ask"),
        "{said}"
    );
}

/// Failure: after unsaved moves the address names the tab's page, so the link opens another page than the one meant, or the person isn't told that it's the
/// saved file's page. Page 1 moved to 3: the tab's page 3 is the file's page 1.
#[test]
fn after_an_unsaved_move_the_address_names_the_files_page_and_says_so() {
    let (app, _file) = open_with("f.pdf", 5);
    app.person(r##"{"cmd":"page_move","tab":"#1","pages":"1","to":3}"##);
    app.settle();
    app.act(UiAction::CopyLink { row: contents_row(&app, 3) });
    app.settle();
    assert!(pasteboard(&app).expect("a copy").ends_with("#page=1"));
    let said = app.core().window.notices().notice().expect("said");
    assert!(said.contains("page 1") && said.contains("saved file"), "{said}");
}

/// Failure: Copy Link is missing from the right-click menu, or is put somewhere other than just after Copy.
#[test]
fn copy_link_is_in_the_context_menu_after_copy() {
    let ids = |entries: Vec<crate::menus::MenuEntry>| entries.into_iter().map(|entry| entry.id).collect::<Vec<_>>();
    let edit = ids(crate::menus::context_menu(None, None));
    let at = edit.iter().position(|id| id == "edit.copy").expect("Copy");
    assert_eq!(edit[at + 1], ID);
}

/// Failure: a contents row with no page offers Copy Link (the menu for a row that goes nowhere), or one with a page offers nothing or more than Rust's list:
/// the host drew what Rust decided, and Rust decides here.
#[test]
fn a_contents_rows_menu_is_rusts_copy_link_for_a_row_with_a_page_and_nothing_otherwise() {
    let (app, _file) = open_with("menu.pdf", 2);
    let with = crate::core::copy_link::contents_menu(&contents_row(&app, 1));
    assert_eq!(
        with.iter().map(|entry| (entry.id.as_str(), entry.title.as_str())).collect::<Vec<_>>(),
        [(ID, "Copy Link")]
    );
    let without = SidebarRow {
        page: None,
        ..contents_row(&app, 1)
    };
    assert!(crate::core::copy_link::contents_menu(&without).is_empty());
}
