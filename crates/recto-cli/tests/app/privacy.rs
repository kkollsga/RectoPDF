//! What the app says about privacy (`recto_ffi`'s `privacy` module): the empty window's quiet line about the daily update check, the same fact under the *Update automatically* switch, and the Settings link to `PRIVACY.md`.

use super::common::State;
use super::common::app::{Run, numbered, wait_for};
use recto_update::source::REPO_ENV;
use std::ffi::OsStr;

/// A build that checks by itself, as a release does, against a port nothing answers on: the check waits a minute after launch, and would only fail.
fn checking_run(state: &State) -> Option<Run<'_>> {
    Run::on_with_env(state, &[(REPO_ENV, OsStr::new("http://127.0.0.1:9"))])
}

/// Failure: a build that asks GitHub every day by itself says nothing about it before the first check, or says it over a page, or out of the empty window's sight; the switch reads as if turning it off stops the check; or Settings has no way to the statement, or one that goes somewhere else.
#[test]
fn a_build_that_checks_says_so_quietly_and_links_the_statement() {
    let state = State::new();
    let Some(run) = checking_run(&state) else { return };
    run.settled();
    let app = run.report()["app"].clone();
    assert_eq!(app["home"], true, "{app}");
    let note = &app["home_note"];
    let text = note["text"].as_str().unwrap_or_default().to_lowercase();
    assert!(text.contains("once a day") && text.contains("github"), "{note}");
    assert!(text.contains("nothing about your documents"), "{note}");
    assert_eq!(note["in_home"], true, "in the empty window, whole: {note}");
    let file = numbered(2);
    run.ok(&["open", file.0.to_str().unwrap()]);
    wait_for(&run, |state| state["app"]["home"] == false);

    run.ok(&["qa", "settings", "open"]);
    let settings = wait_for(&run, |state| state["app"]["settings"]["open"] == true)["app"]["settings"].clone();
    let switch = settings["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["title"] == "Update automatically")
        .cloned()
        .unwrap();
    let hint = switch["hint"].as_str().unwrap().to_lowercase();
    assert!(hint.contains("off, it still asks github once a day"), "{switch}");
    let url = settings["privacy"]["url"].as_str().unwrap().to_owned();
    assert_eq!(url, format!("{}/blob/main/PRIVACY.md", recto_update::REPO), "{settings}");
    assert!(!settings["privacy"]["title"].as_str().unwrap_or_default().is_empty(), "{settings}");
    run.ok(&["qa", "settings", "privacy"]);
    let opened = wait_for(&run, |state| !state["app"]["links"]["address_opened"].is_null());
    assert_eq!(
        opened["app"]["links"]["address_opened"],
        url.as_str(),
        "the link opens the statement, as the person chose"
    );
    run.quit();
}

/// Failure: a local build, which checks only when asked, tells the reader it asks GitHub every day.
#[test]
fn a_local_build_says_nothing_of_a_check_it_doesnt_make() {
    let Some(run) = Run::start() else { return };
    run.settled();
    let app = run.report()["app"].clone();
    assert_eq!(app["home"], true, "{app}");
    assert!(app["home_note"].is_null(), "{app}");
}
