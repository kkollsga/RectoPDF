//! **Highlights through the app core, with no window** (R4.3, `docs/FEATURES.md` step 10): Mark Up > Highlight as the command an agent sends, what the
//! page view is given to draw, an agent's highlight at Control as its own step, and what a Save does to them and to the history (the coordinator's E1).

use super::twins::{Twins, names};
use crate::core::controls::UiAction;
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::Level;
use recto_reader::tiles::Op;
use serde_json::Value;

/// The highlights the reading page view was last given: the whole set's pages, each with how many lines, from the ops it hasn't handed to a host.
fn given(app: &HeadlessApp) -> Option<Vec<(u32, usize)>> {
    assert!(
        app.core().core.flush(std::time::Duration::from_secs(10)),
        "the session thread took what was queued"
    );
    let ops = app.core().panes.reading_engine(&app.core().engine).drain();
    ops.into_iter().rev().find_map(|op| match op {
        Op::Marks { pages, .. } => Some(pages.into_iter().map(|(page, rects)| (page, rects.len())).collect()),
        _ => None,
    })
}

fn selects(app: &HeadlessApp, from: (u32, u32), to: (u32, u32)) {
    app.core().panes.reading_engine(&app.core().engine).selection().restore(from, to);
}

fn menu(id: &str) -> UiAction {
    UiAction::Menu { id: id.into() }
}

fn listed(app: &HeadlessApp) -> Value {
    app.agent(Level::ReadOnly, r##"{"cmd":"highlights","tab":"active"}"##).expect("highlights")
}

/// Failure (R2.3's rule, for this feature): Mark Up > Highlight does something other than `recto highlight` of the same characters: another stretch,
/// another event, another step kept; or the person's highlight isn't the owner's. The person's selection on one app, the agent's line at Full on another.
#[test]
fn mark_up_highlight_is_the_agents_command_of_the_selection() {
    let twins = Twins::on(&[("a.pdf", 3)]);
    selects(&twins.person, (0, 0), (0, 5));
    let (said, agent_said) = twins.both(
        menu(crate::core::highlight::HIGHLIGHT_ID),
        r##"{"cmd":"highlight","tab":"active","page":1,"chars":"0-5"}"##,
    );
    assert_eq!(names(&said), ["edits_changed"]);
    assert_eq!(said, agent_said, "the same said");
    twins.assert_same(
        "Highlight and `highlight`",
        &[
            "by",
            "holds_owner_edits",
            "owner_steps_undone",
            "undone_owner_steps",
            "journal",
            "edits_full",
            "selection",
        ],
    );
    let (person, agent) = (listed(&twins.person), listed(&twins.agent));
    assert_eq!(person["edits"][0]["rects"], agent["edits"][0]["rects"], "the same lines");
    assert_eq!(
        (person["edits"][0]["by"].as_str(), agent["edits"][0]["by"].as_str()),
        (Some("user"), Some("agent"))
    );
    // Nothing selected sends nothing.
    let before = twins.person.session_json();
    twins.person.core().panes.reading_engine(&twins.person.core().engine).selection().clear_soon();
    twins.person.settle();
    twins.person.act(menu(crate::core::highlight::REMOVE_ID));
    twins.person.settle();
    assert_eq!(twins.person.session_json()["edits"], before["edits"], "no selection, nothing removed");
}

/// Failure: the page view isn't given a highlight to draw, is given it on another page, keeps drawing it once it's undone, or is given the set again
/// on a pass where nothing of it changed.
#[test]
fn the_page_view_draws_what_the_edits_hold() {
    let disk = Disk::new();
    let doc = disk.pdf("drawn.pdf", 3);
    let app = disk.start();
    app.open(&doc);
    app.settle();
    let _ = given(&app);
    app.agent(Level::Control, r##"{"cmd":"highlight","tab":"active","page":2,"chars":"0-4"}"##)
        .expect("highlight");
    assert_eq!(given(&app), Some(vec![(1, 1)]), "page 2's one line");
    app.agent(Level::Control, r#"{"cmd":"undo"}"#).expect("undo");
    assert_eq!(given(&app), Some(vec![]), "undone: cleared");
    app.agent(Level::Control, r#"{"cmd":"goto","page":2}"#).expect("a move");
    assert_eq!(given(&app), None, "nothing new of them, nothing given");
}

/// Failure (E1, coordinator 2026-11-14): a Save leaves a highlight as an unsaved edit, doesn't write it, or leaves its step for Undo to take back as if
/// it weren't in the file; Undo stays enabled with nothing to undo; an agent's `undo` says it undid something; or a Save that failed drops a step.
#[test]
fn a_save_writes_the_highlight_and_undo_goes_past_it() {
    let disk = Disk::new();
    let doc = disk.pdf("saved.pdf", 2);
    let app = disk.start();
    app.open(&doc);
    app.settle();
    app.agent(Level::Control, r##"{"cmd":"highlight","tab":"active","page":1,"chars":"0-5"}"##)
        .expect("highlight");
    app.settle();
    // A Save refused (the file changed on disk under the edits) drops nothing.
    let bytes = std::fs::read(&doc).unwrap();
    std::fs::write(&doc, [&bytes[..], b"\n%changed\n"].concat()).unwrap();
    assert!(
        app.agent(Level::Control, r##"{"cmd":"save","tab":"active"}"##).is_err(),
        "a changed file isn't saved over"
    );
    app.settle();
    assert_eq!(listed(&app)["edits"].as_array().map(Vec::len), Some(1), "the highlight's still an edit");
    assert!(app.core().menu_item_enabled("edit.undo".into()), "and can be undone");
    std::fs::write(&doc, &bytes).unwrap();
    app.agent(Level::Full, r##"{"cmd":"discard","tab":"active"}"##).expect("discard");
    app.agent(Level::Control, r##"{"cmd":"close","tab":"active"}"##).expect("close");
    app.open(&doc);
    app.settle();
    app.agent(Level::Control, r##"{"cmd":"highlight","tab":"active","page":1,"chars":"0-5"}"##)
        .expect("highlight again");
    app.agent(Level::Control, r##"{"cmd":"save","tab":"active"}"##).expect("save");
    app.settle();
    let read = recto_core::highlights::page_highlights(&doc, 0).expect("the saved file reads");
    assert_eq!(read.found.len(), 1, "written into the file");
    assert_eq!(listed(&app)["edits"].as_array().map(Vec::len), Some(0), "and no longer an edit");
    assert!(
        !app.core().menu_item_enabled("edit.undo".into()),
        "nothing to undo: the highlight's step went with the Save"
    );
    let undo = app.agent(Level::Control, r#"{"cmd":"undo"}"#);
    assert!(
        undo.is_err_and(|error| error.code == recto_agent::Code::NOTHING_TO_UNDO),
        "an agent's undo says so"
    );
}

/// Failure: below Full an agent's highlight throws away the user's undone edits, which the edit path's rule keeps; or an agent's own highlight can't
/// be taken back by it.
#[test]
fn an_agent_below_full_keeps_the_users_undone_edits() {
    let disk = Disk::new();
    let doc = disk.pdf("kept.pdf", 2);
    let app = disk.start();
    app.open(&doc);
    app.settle();
    selects(&app, (0, 0), (0, 4));
    app.act(menu(crate::core::highlight::HIGHLIGHT_ID));
    app.settle();
    app.act(menu("edit.undo"));
    app.settle();
    let refused = app.agent(Level::Control, r##"{"cmd":"highlight","tab":"active","page":2,"chars":"0-4"}"##);
    assert!(
        refused.is_err_and(|error| error.code == recto_agent::Code::WOULD_CLEAR_REDO),
        "the user's undone highlight stays to redo"
    );
    app.agent(Level::Control, r#"{"cmd":"redo"}"#).expect("redo keeps everything");
    app.agent(Level::Control, r##"{"cmd":"highlight","tab":"active","page":2,"chars":"0-4"}"##)
        .expect("then it may");
    app.agent(Level::Control, r##"{"cmd":"highlight_remove","tab":"active","id":1}"##)
        .expect("and take its own away");
    assert_eq!(listed(&app)["edits"].as_array().map(Vec::len), Some(1));
}

/// Failure: asking to take away a highlight that's already in the file does nothing and says nothing, or says there's none there.
#[test]
fn removing_a_files_own_highlight_says_it_cant_yet() {
    let disk = Disk::new();
    let doc = disk.pdf("file.pdf", 1);
    let app = disk.start();
    app.open(&doc);
    app.settle();
    app.agent(Level::Control, r##"{"cmd":"highlight","tab":"active","page":1,"chars":"0-5"}"##)
        .expect("highlight");
    app.agent(Level::Control, r##"{"cmd":"save","tab":"active"}"##).expect("save");
    app.settle();
    let file = listed_on(&app, 1);
    let [x, y, width, height] = ["0", "1", "2", "3"].map(|at| file["file"]["highlights"][0]["rects"][0][at.parse::<usize>().unwrap()].as_f64().unwrap());
    let at = format!("{},{}", x + width / 2.0, y + height / 2.0);
    let refused = app.recto(Level::Control, &["highlight", "remove", "--page", "1", "--at", &at]);
    assert!(refused.is_err_and(|error| error.code == recto_agent::Code::HIGHLIGHT_IN_FILE));
}

fn listed_on(app: &HeadlessApp, page: u32) -> Value {
    app.agent(Level::ReadOnly, &format!(r##"{{"cmd":"highlights","tab":"active","page":{page}}}"##))
        .expect("highlights")
}
