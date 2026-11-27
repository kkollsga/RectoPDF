//! **A link pasted into ⌘K**, through the app core with no window: the first row is the link's open, chosen as the person's `open {path, page}` (the one a link
//! from outside is, without its cap) and as an agent's own `open`.

use super::changed_under_edits::pdf_bytes;
use crate::RowKind;
use crate::core::controls::UiAction;
use crate::palette::PaletteRow;
use crate::testkit::headless::HeadlessApp;
use recto_agent::Level;
use std::path::Path;

fn link_to(path: &Path, page: u32) -> String {
    recto_core::locator::address_of(path, page).expect("a link")
}

/// The rows the palette draws with `text` typed in its field, as the person sees them.
fn rows_for(app: &HeadlessApp, text: &str) -> Vec<PaletteRow> {
    if !app.core().tools.palette().is_open() {
        app.core().tools.palette().open();
    }
    crate::core::palette::typed(app.core(), text);
    crate::core::palette::view(app.core()).rows
}

fn page(app: &HeadlessApp) -> u64 {
    app.recto(Level::ReadOnly, &["where"]).expect("where")["page"].as_u64().expect("a page")
}

fn tab_count(app: &HeadlessApp) -> usize {
    app.tabs().iter().filter(|row| row.kind == RowKind::Tab).count()
}

fn start_with(name: &str, pages: usize) -> (HeadlessApp, std::path::PathBuf) {
    let app = HeadlessApp::start();
    let file = app.disk().file(name, &pdf_bytes("Mine", pages));
    (app, file)
}

/// Failure: a pasted link isn't offered first, is worded differently from Rust's words, or choosing it doesn't land on the page: the person pastes a link to
/// page 3 of a closed file, sees "Open link: …, page 3" before anything else, and Return opens that file at page 3. A link with no page says no page.
#[test]
fn a_pasted_link_is_the_first_row_and_opens_at_its_page() {
    let (app, file) = start_with("pasted.pdf", 5);
    let rows = rows_for(&app, &link_to(&file, 3));
    assert_eq!(rows[0].title, "Open link: pasted.pdf, page 3", "{rows:?}");
    assert!(rows[0].enabled);
    let bare = link_to(&file, 3).split('#').next().unwrap().to_string();
    assert_eq!(rows_for(&app, &bare)[0].title, "Open link: pasted.pdf");
    let id = rows_for(&app, &link_to(&file, 3))[0].id.clone();
    app.act(UiAction::PaletteRun { id });
    app.settle();
    assert_eq!((tab_count(&app), page(&app)), (1, 3));
    assert!(!app.core().tools.palette().is_open(), "the palette closed");
}

/// Failure: the row opens the page of the tab rather than of the saved file, so after unsaved edits the person lands on another page than the link names,
/// or an already-open file opens again in a second tab. With page 2 deleted, a pasted link to file page 3 lands on the tab's page 2; with the file open, no
/// second tab.
#[test]
fn choosing_it_is_the_link_open_mapped_through_edits_and_comes_to_the_open_tab() {
    let (app, file) = start_with("edited.pdf", 4);
    app.open(&file);
    app.person(r##"{"cmd":"page_delete","tab":"#1","pages":"2"}"##);
    app.settle();
    let id = rows_for(&app, &link_to(&file, 3))[0].id.clone();
    app.act(UiAction::PaletteRun { id });
    app.settle();
    assert_eq!((tab_count(&app), page(&app)), (1, 2));
    let said = app.core().window.notices().notice().expect("it says the renumbering");
    assert!(said.contains("Page 3 of the saved file is page 2 here"), "{said}");
}

/// Failure: a link that can't be read opens something, is hidden, or its row repeats part of the link (a page's text in front of the person), or choosing it
/// closes the palette or opens a tab. Each unreadable link gives one row that can't be chosen, in Rust's words, with none of the link in it; running it does
/// nothing. Plain words give no link row.
#[test]
fn a_bad_link_is_one_disabled_row_in_rusts_words_and_plain_words_add_none() {
    let (app, file) = start_with("bad.pdf", 2);
    for bad in ["rectopdf:///a/%2E%2E/MARKERb.pdf", "rectopdf:///MARKERx.pdf#page=0"] {
        let rows = rows_for(&app, bad);
        assert!(!rows[0].enabled, "{rows:?}");
        assert!(
            rows[0].id.starts_with("link:") && !rows[0].id.contains("MARKER") && !rows[0].title.contains("MARKER"),
            "{rows:?}"
        );
        assert!(rows[0].title.contains("This link"), "{rows:?}");
        app.act(UiAction::PaletteRun { id: rows[0].id.clone() });
        app.settle();
        assert_eq!(tab_count(&app), 0);
        assert!(app.core().tools.palette().is_open(), "choosing it leaves the palette as it was");
    }
    for plain in ["zoom", "http://example.com/a.pdf", file.to_str().unwrap()] {
        assert!(rows_for(&app, plain).iter().all(|row| !row.id.starts_with("link:")), "{plain}");
    }
}

/// Failure: a very long or odd file name fills the row or carries control characters into it, or the field's own limit cuts the pasted link short so it names
/// another file (the first version showed a name cut at 39 letters, with no page). The link is kept whole: the name is cleaned and cut at about 60 characters
/// with "…", and the page is still said.
#[test]
fn a_long_file_name_is_cut_and_cleaned() {
    let (app, _) = start_with("x.pdf", 1);
    let long = format!("{}.pdf", "n".repeat(100));
    let path = app.disk().files().join(&long);
    let title = rows_for(&app, &link_to(&path, 1))[0].title.clone();
    assert!(title.contains('…') && title.chars().count() < 100 && title.ends_with(", page 1"), "{title}");
}

/// Failure: an agent's `palette --run` of the link row runs as the person, or lets an agent below Control open a file, or opens at the wrong page. At
/// Read-only it's refused and nothing opens; at Control it's the agent's own `open`, at the page.
#[test]
fn an_agents_run_of_the_link_row_is_its_own_open_refused_below_control() {
    let (app, file) = start_with("agent.pdf", 5);
    let query = serde_json::to_string(&link_to(&file, 4)).unwrap();
    let line = format!(r#"{{"cmd":"palette","query":{query},"run":true}}"#);
    app.agent(Level::ReadOnly, &line).expect_err("running is doing");
    app.settle();
    assert_eq!(tab_count(&app), 0);
    let ran = app.agent(Level::Control, &line).expect("at Control");
    assert!(ran["ran"]["id"].as_str().is_some_and(|id| id.starts_with("link:")), "{ran}");
    app.settle();
    assert_eq!((tab_count(&app), page(&app)), (1, 4));
}

/// Failure: an agent's `palette --run` that fails closes the person's open palette (the close ran before the command did), losing what they typed. With the
/// palette open on a typed query, an agent at Control runs a link row to a file that isn't there: the open fails, and the palette is still open with its query.
#[test]
fn a_failed_agent_run_leaves_the_persons_palette_open() {
    let (app, _file) = start_with("kept.pdf", 3);
    let missing = app.disk().files().join("gone.pdf");
    let link = link_to(&missing, 2);
    rows_for(&app, &link);
    let query = serde_json::to_string(&link).unwrap();
    app.agent(Level::Control, &format!(r#"{{"cmd":"palette","query":{query},"run":true}}"#))
        .expect_err("the open fails");
    assert!(app.core().tools.palette().is_open(), "the person's palette is still open");
    assert_eq!(crate::core::palette::view(app.core()).text, link);
}

/// Failure: an agent running the row of a link that can't be read gets a generic "unknown id", or words that hold part of the link. It gets the link's own
/// refusal, as `open --link` would give it: the code and Rust's constant words, none of the link.
#[test]
fn an_agent_running_a_refused_link_gets_the_links_own_refusal() {
    let (app, _file) = start_with("r.pdf", 2);
    for (bad, code) in [("rectopdf:///a/%2E%2E/MARKERb.pdf", "bad_path"), ("rectopdf:///MARKERx.pdf#page=0", "bad_page")] {
        let query = serde_json::to_string(bad).unwrap();
        let error = app
            .agent(Level::Control, &format!(r#"{{"cmd":"palette","query":{query},"run":true}}"#))
            .expect_err("refused");
        assert_eq!(error.code.to_string(), code, "{error:?}");
        let words = recto_core::locator::read_address(bad).expect_err("unreadable").message();
        assert_eq!(error.message, words);
        assert!(!error.message.contains("MARKER"));
    }
    assert_eq!(tab_count(&app), 0);
}
