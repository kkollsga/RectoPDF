//! What VoiceOver and the system's display settings get from the app's own views: ⌘K's rows as a list, the sidebar's thumbs as scroll bars, a contents section's fold as an action and a key, and the thumbs with Increase Contrast. The words and the rules are `recto-ffi`'s (`palette.rs`, `sidebar_access.rs`), unit-tested there; these check the app hands them to the system, through `qa accessibility`, which calls what the system calls, with nothing posted.

use super::{Run, numbered, sectioned, wait_for};
use serde_json::Value;

/// Failure: VoiceOver hears ⌘K's rows as a table of one column, or as nothing, or a row without its menu and key, or isn't told which row Return runs as ↑ and ↓ move it.
#[test]
fn the_palettes_rows_are_a_list_to_voiceover() {
    let Some(run) = Run::start() else { return };
    let file = numbered(4);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "menu", "palette.open"]);
    wait_for(&run, |state| state["app"]["palette"]["open"] == true);
    let typed = run.ok(&["qa", "palette-type", "zoom"]);
    let access = &typed["palette"]["accessibility"];
    assert_eq!(access["role"], "AXList", "{access}");
    assert!(!access["label"].as_str().unwrap().is_empty(), "the list has a name: {access}");
    let rows = access["rows"].as_array().unwrap();
    assert!(rows.len() >= 2, "{access}");
    let first = &typed["palette"]["rows"][0];
    assert_eq!(rows[0]["element"], true, "{access}");
    let said = rows[0]["label"].as_str().unwrap();
    assert!(
        said.starts_with(first["title"].as_str().unwrap()) && said.contains(first["detail"].as_str().unwrap()),
        "a row is said with where it lives: {said:?} for {first}"
    );
    assert_eq!(access["selected_rows"], serde_json::json!([0]), "{access}");
    assert_eq!(access["selected_labels"], serde_json::json!([said]), "{access}");
    let moved = run.ok(&["qa", "palette-key", "down"]);
    let access = &moved["palette"]["accessibility"];
    assert_eq!(
        access["selected_rows"],
        serde_json::json!([1]),
        "↓ moves what VoiceOver is told is selected: {access}"
    );
    assert_eq!(access["selected_labels"][0], access["rows"][1]["label"], "{access}");
    run.ok(&["qa", "palette-key", "escape"]);
}

fn contents(run: &Run<'_>) -> Value {
    run.report()["app"]["contents"].clone()
}

/// Failure: the sidebar's own thumb is nothing to VoiceOver, or says no place or a wrong one, or can't be moved without a pointer; or a section can be folded only by the pointer, as its fold shows only under it, by neither an accessibility action nor a key; or a row doesn't say whether its sections show.
#[test]
fn the_sidebars_thumb_and_folds_work_without_a_pointer() {
    let Some(run) = Run::start() else { return };
    let file = sectioned();
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let open = contents(&run);
    let thumb = &open["thumb"]["accessibility"];
    assert_eq!(
        (thumb["role"].as_str(), thumb["orientation"].as_str(), thumb["value"].as_f64()),
        (Some("AXScrollBar"), Some("vertical"), Some(0.0)),
        "{open}"
    );
    assert!(
        !thumb["label"].as_str().unwrap().is_empty() && !thumb["value_text"].as_str().unwrap().is_empty(),
        "{thumb}"
    );
    // A page down, as VoiceOver's increment does, and back.
    run.ok(&["qa", "accessibility", "thumb:contents:increment"]);
    let down = wait_for(&run, |state| {
        state["app"]["contents"]["thumb"]["accessibility"]["value"].as_f64().unwrap_or(0.0) > 0.0
    });
    let scrolled = down["app"]["contents"]["scroll_y"].as_f64().unwrap();
    assert!(scrolled > 0.0, "{}", down["app"]["contents"]);
    run.ok(&["qa", "accessibility", "thumb:contents:decrement"]);
    wait_for(&run, |state| state["app"]["contents"]["thumb"]["accessibility"]["value"].as_f64() == Some(0.0));
    // Chapter 0 is open by following, with its fold as an action.
    let rows = contents(&run)["accessibility"].clone();
    let chapter = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["row"] == 0)
        .cloned()
        .unwrap_or_else(|| panic!("{rows}"));
    assert_eq!(chapter["expanded"], true, "{chapter}");
    let actions = chapter["actions"].as_array().unwrap().clone();
    assert_eq!(actions.len(), 1, "{chapter}");
    assert!(chapter["label"].as_str().unwrap().starts_with("Chapter 0"), "{chapter}");
    // A section row has nothing under it, and no fold.
    let section = rows.as_array().unwrap().iter().find(|row| row["row"] == 1).cloned().unwrap();
    assert!(section["actions"].as_array().unwrap().is_empty(), "{section}");
    run.ok(&["qa", "accessibility", "fold:0"]);
    let folded = wait_for(&run, |state| state["app"]["contents"]["count"] == 40);
    let chapter = folded["app"]["contents"]["accessibility"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["row"] == 0)
        .cloned()
        .unwrap();
    assert_eq!(chapter["expanded"], false, "{chapter}");
    assert_ne!(chapter["actions"][0], actions[0], "the action says it opens now: {chapter}");
    // → opens the section being read and ← folds it, as the sidebar's key does.
    run.ok(&["qa", "accessibility", "key:right"]);
    wait_for(&run, |state| state["app"]["contents"]["count"] == 41);
    run.ok(&["qa", "accessibility", "key:left"]);
    wait_for(&run, |state| state["app"]["contents"]["count"] == 40);
    let refused = run.state.json(&["qa", "accessibility", "key:up"]);
    assert_ne!(refused.1, 0, "{}", refused.0);
}

/// Failure: with the system's Increase Contrast on, the sidebar's and the page area's thumbs are drawn as faint as ever, or stay strong once it's off again, on light or dark.
#[test]
fn increase_contrast_strengthens_the_thumbs() {
    let Some(run) = Run::start() else { return };
    let file = sectioned();
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    for look in ["light", "dark"] {
        run.ok(&["qa", "appearance", look]);
        run.settled();
        let alphas = |state: &Value| {
            (
                state["app"]["contents"]["thumb"]["alpha"].as_f64().unwrap(),
                state["app"]["pages"]["thumb"]["alpha"].as_f64().unwrap(),
            )
        };
        let usual = alphas(&run.report());
        run.ok(&["qa", "accessibility", "contrast:on"]);
        let strong = run.report();
        assert_eq!(strong["app"]["contents"]["thumb"]["increase_contrast"], true, "{look}");
        let (contents, pages) = alphas(&strong);
        assert!(
            contents > usual.0 * 1.5 && pages > usual.1 * 1.5,
            "{look}: {usual:?} then {:?}",
            (contents, pages)
        );
        run.ok(&["qa", "accessibility", "contrast:off"]);
        assert_eq!(alphas(&run.report()), usual, "{look}: back as it was");
    }
}
