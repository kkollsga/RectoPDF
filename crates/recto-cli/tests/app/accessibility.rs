//! What VoiceOver gets from the app's own views: ⌘K's rows as a list. The words are `recto-ffi`'s (`palette.rs`), unit-tested there; these check the app hands them to the system, read back from the views themselves.

use super::{Run, numbered, wait_for};

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
    assert_eq!(access["selected_rows"], serde_json::json!([1]), "↓ moves what VoiceOver is told is selected: {access}");
    assert_eq!(access["selected_labels"][0], access["rows"][1]["label"], "{access}");
    run.ok(&["qa", "palette-key", "escape"]);
}
