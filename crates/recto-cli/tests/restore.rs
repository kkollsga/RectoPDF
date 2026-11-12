//! The state record end to end: the real app in quiet runs, quit and started again on the same temp state folder. SPEC decision 23: "It's crucial that app state is perfectly recovered before and after update".
//!
//! The first test here is the one that matters. It compares the whole of `qa state` before and after, with a short named list of exceptions, and a list of every path that's known about, so a field added to `qa state` fails until someone says here whether it's carried over or an exception. State is complete by construction, and nothing can be quietly forgotten.

mod common;

use common::State;
pub(crate) use common::app::{Run, app_path, chaptered, long_document};
use recto_agent::Level;
use recto_platform::local::process_counters;
use recto_testpdf::{Page, TempPdf, form_bomb, pages, text_lines};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// Every value in a report by its path, with lists' places as `[]` in what's compared by name and their number in the value's path.
fn flatten(value: &Value, path: &str, out: &mut BTreeMap<String, Value>) {
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                let inner = if path.is_empty() { key.clone() } else { format!("{path}.{key}") };
                flatten(item, &inner, out);
            }
        }
        Value::Array(list) => {
            out.insert(format!("{path}.len"), json!(list.len()));
            for (at, item) in list.iter().enumerate() {
                flatten(item, &format!("{path}[{at}]"), out);
            }
        }
        other => {
            out.insert(path.to_string(), other.clone());
        }
    }
}

/// A path with its list places and tab numbers taken out, as a list of known paths has it.
fn general(path: &str) -> String {
    let mut out = String::new();
    let mut digits = false;
    let mut in_list = false;
    for c in path.chars() {
        match c {
            '[' => {
                in_list = true;
                out.push_str("[]");
            }
            ']' => in_list = false,
            _ if in_list => {}
            c if c.is_ascii_digit() && (digits || out.ends_with('.')) => digits = true,
            c => {
                if digits {
                    out.push('N');
                }
                digits = false;
                out.push(c);
            }
        }
    }
    if digits {
        out.push('N');
    }
    out
}

/// What's allowed to differ across a restart, and why: one file per area under `restore/exceptions/`, each line a path, or the start of one, then its reason. It's short on purpose: anything else that differs is a bug, or goes here with its reason. Files, sorted, so branches adding to different places merge without a conflict (review T3).
const EXCEPTIONS: &[(&str, &str)] = &[
    ("app", include_str!("restore/exceptions/app.txt")),
    ("hangs", include_str!("restore/exceptions/hangs.txt")),
    ("memory", include_str!("restore/exceptions/memory.txt")),
    ("panes", include_str!("restore/exceptions/panes.txt")),
    ("printing", include_str!("restore/exceptions/printing.txt")),
    ("reader", include_str!("restore/exceptions/reader.txt")),
    ("record", include_str!("restore/exceptions/record.txt")),
    ("slow", include_str!("restore/exceptions/slow.txt")),
    ("snapshot", include_str!("restore/exceptions/snapshot.txt")),
    ("update", include_str!("restore/exceptions/update.txt")),
];

/// Every path in `qa state` that's been looked at: one file per area under `restore/known/`, sorted. Each comes back, as the test of everything shows, or it's in [`EXCEPTIONS`]. A new one fails until it's added, which is the moment to carry it over.
const KNOWN_PATHS: &[(&str, &str)] = &[
    ("app", include_str!("restore/known/app.txt")),
    ("panes", include_str!("restore/known/panes.txt")),
    ("printing", include_str!("restore/known/printing.txt")),
    ("reader", include_str!("restore/known/reader.txt")),
    ("session", include_str!("restore/known/session.txt")),
    ("snapshot", include_str!("restore/known/snapshot.txt")),
];

fn known_paths() -> Vec<&'static str> {
    KNOWN_PATHS.iter().flat_map(|(_, list)| list.lines()).collect()
}

/// Each exception as `(path, why)`.
fn exceptions() -> Vec<(&'static str, &'static str)> {
    EXCEPTIONS
        .iter()
        .flat_map(|(_, list)| list.lines())
        .map(|line| line.split_once(' ').unwrap_or((line, "")))
        .collect()
}

/// Failure (full check, 2026-11-12, `an_update_in_the_middle_of_things_changes_nothing`): the whole-state comparison failed with `app.animations: nothing then "chrome_fading"`. The zoom level's and page number's fade starts on its own timer 3 s after their last change (`ChromeFader.armTimer`, `Chrome.swift`), and `wait-settled` only waits for a fade already under way; on a loaded machine the 3 s ran out between `settled` and the state read (the test took 65 s). It is not the pointer: a quiet run's `pointer_in` stayed empty. A fade in flight is not part of what comes back, as `app.chrome` isn't, so two states that differ only in it are the same. Needs no app.
#[test]
fn a_fade_that_began_on_its_own_timer_is_not_a_difference() {
    let at_rest = json!({ "app": { "animations": [], "pages": { "scroll_y": 12.0 } } });
    let fading = json!({ "app": { "animations": ["chrome_fading"], "pages": { "scroll_y": 12.0 } } });
    assert_same(&at_rest, &fading, &[]);
    assert_same(&fading, &at_rest, &[]);
    // Any other animation is still one, and so is a real difference beside a fade.
    let zooming = json!({ "app": { "animations": ["zoom_settling"], "pages": { "scroll_y": 12.0 } } });
    assert!(std::panic::catch_unwind(|| assert_same(&at_rest, &zooming, &[])).is_err());
    let moved = json!({ "app": { "animations": ["chrome_fading"], "pages": { "scroll_y": 13.0 } } });
    assert!(std::panic::catch_unwind(|| assert_same(&at_rest, &moved, &[])).is_err());
}

/// Failure (review T3): the list of known paths was unsorted, with new ones added at its top, so every branch that added a `qa state` field conflicted there. Each area's file, of known paths and of exceptions, is sorted with no line twice, holds only its own area's paths, and every file in those folders is read. An exception says why. Needs no app.
#[test]
fn the_known_paths_and_exceptions_are_sorted_with_no_duplicates() {
    for (folder, lists) in [("known", KNOWN_PATHS), ("exceptions", EXCEPTIONS)] {
        for (area, list) in lists {
            let file = format!("restore/{folder}/{area}.txt");
            let lines: Vec<(&str, &str)> = list.lines().map(|line| line.split_once(' ').unwrap_or((line, ""))).collect();
            assert!(!lines.is_empty(), "{file} is empty");
            for pair in lines.windows(2) {
                assert!(
                    pair[0].0 < pair[1].0,
                    "{file}: `{}` then `{}`. Each path once, in sorted order (`LC_ALL=C sort`)",
                    pair[0].0,
                    pair[1].0
                );
            }
            for (path, why) in &lines {
                assert!(
                    path.strip_prefix(area).is_some_and(|rest| rest.is_empty() || rest.starts_with(['.', '['])),
                    "{file}: `{path}` belongs in another area's file"
                );
                assert_eq!(
                    folder == "exceptions",
                    !why.trim().is_empty(),
                    "{file}: `{path}`: an exception says why, and a known path is a path alone"
                );
            }
        }
        let folder_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/restore").join(folder);
        let mut on_disk: Vec<String> = std::fs::read_dir(&folder_path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        on_disk.sort();
        let mut read: Vec<String> = lists.iter().map(|(area, _)| format!("{area}.txt")).collect();
        read.sort();
        assert_eq!(on_disk, read, "every file in restore/{folder} is read, and nothing else is there");
    }
}

/// What can be compared as numbers. Scroll positions can differ by rounding only: the test below of pixels holds them much closer than any eye.
fn differs(left: &Value, right: &Value) -> bool {
    match (left.as_f64(), right.as_f64()) {
        (Some(left), Some(right)) => (left - right).abs() > 1e-6,
        _ => left != right,
    }
}

/// A state without the zoom level's and page number's fade in `app.animations`, which starts on a timer of its own.
fn without_timed_fade(state: &Value) -> Value {
    let mut state = state.clone();
    if let Some(list) = state["app"]["animations"].as_array_mut() {
        list.retain(|one| one.as_str() != Some("chrome_fading"));
    }
    state
}

/// Compares everything, and fails on anything but what's named.
fn assert_same(before: &Value, after: &Value, known: &[&str]) {
    assert_same_but(before, after, known, &[]);
}

/// The same, with what else a caller can name and say why.
fn assert_same_but(before: &Value, after: &Value, known: &[&str], also: &[&str]) {
    let (mut one, mut two) = (BTreeMap::new(), BTreeMap::new());
    // A fade the 3 s timer starts can begin between `settled` and the state read, on either side: it's no more part of what comes back than `app.chrome` is (and as `moving_besides_the_timed_fade` in tests/app.rs has it).
    flatten(&without_timed_fade(before), "", &mut one);
    flatten(&without_timed_fade(after), "", &mut two);
    let exceptions = exceptions();
    let excepted =
        |path: &str| exceptions.iter().any(|(name, _)| path.starts_with(name)) || also.iter().any(|name| path.starts_with(name)) || ci_known::covers(path);
    let (mut wrong, mut let_through) = (Vec::new(), Vec::new());
    for (path, value) in &one {
        let said = match two.get(path) {
            Some(other) if !differs(value, other) => continue,
            other => format!("{path}: {value} then {}", other.map_or("nothing".to_string(), ToString::to_string)),
        };
        if !excepted(path) {
            wrong.push(said);
        } else if ci_known::covers(path) {
            let_through.push(said);
        }
    }
    ci_known::say(&let_through);
    for path in two.keys() {
        if !excepted(path) && !one.contains_key(path) {
            wrong.push(format!("{path}: nothing then {}", two[path]));
        }
    }
    // A place that moved is easier to tell apart from one said another way with where the view was, and how tall the document, on each side.
    let view = |state: &Value| json!({ "scroll_y": state["reader"]["scroll_y"], "document_height": state["reader"]["document_height"], "places": state["session"]["tabs"].as_array().map(|tabs| tabs.iter().map(|tab| tab["place"].clone()).collect::<Vec<_>>()) });
    let views = if wrong.iter().any(|line| line.contains(".place.")) {
        format!("\nbefore: {}\nafter: {}", view(before), view(after))
    } else {
        String::new()
    };
    assert!(wrong.is_empty(), "these changed across the restart:\n{}{views}", wrong.join("\n"));
    if !known.is_empty() {
        let unknown: Vec<String> = one
            .keys()
            .map(|path| general(path))
            .filter(|path| !known.contains(&path.as_str()) && !excepted(path))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        assert!(
            unknown.is_empty(),
            "`qa state` has fields this test hasn't been told about. Carry each one over in the state record, or name it in restore/exceptions/ with why, then add it to its area's file in restore/known/, in sorted order:\n{}",
            unknown.join("\n")
        );
    }
}

/// Puts the app in a rich state: several tabs at various zoom and pages and two of them side by side, folded contents, a wide sidebar, Find open with text typed, edit mode, unsaved rotate and delete with undo history by both owner and agent.
fn make_rich(run: &Run<'_>, files: &[&TempPdf]) {
    for file in files {
        run.ok(&["open", file.0.to_str().unwrap()]);
    }
    run.settled();
    // The first tab: far along, zoomed in and scrolled within the page and across it, with edits by both.
    run.ok(&["tap", "#1"]);
    run.settled();
    run.ok(&["goto", "40"]);
    run.settled();
    // A way back and a way forward, which come back too.
    run.ok(&["goto", "20"]);
    run.settled();
    run.ok(&["back"]);
    run.settled();
    run.ok(&["qa", "zoom", "1.7"]);
    run.settled();
    run.ok(&["qa", "scroll", "137"]);
    run.settled();
    run.ok(&["qa", "owner-edit", "rotate:2"]);
    run.ok(&["page", "delete", "5"]);
    // A move and a PDF added, which a list has to hold too.
    run.ok(&["page", "move", "6", "--to", "2"]);
    run.ok(&["page", "insert", files[1].0.to_str().unwrap(), "--at", "3"]);
    run.ok(&["qa", "owner-edit", "delete:9"]);
    run.ok(&["page", "rotate", "3", "--by", "90"]);
    run.ok(&["undo"]);
    run.ok(&["qa", "owner-edit", "undo"]);
    run.settled();
    // The second: another zoom, and folded.
    run.ok(&["tap", "#2"]);
    run.settled();
    run.ok(&["view", "--layout", "two-up-cover"]);
    run.settled();
    run.ok(&["goto", "11"]);
    run.settled();
    run.ok(&["qa", "zoom", "0.8"]);
    run.settled();
    run.ok(&["qa", "scroll", "60"]);
    run.settled();
    run.ok(&["qa", "click-row", "1"]);
    run.settled();
    // The third, which stays active: its contents scrolled by the reader.
    run.ok(&["tap", "#3"]);
    run.settled();
    // Side by side on the tab that stays showing, so its pixel scroll is about that layout.
    run.ok(&["view", "--layout", "two-up"]);
    run.settled();
    run.ok(&["goto", "120"]);
    run.settled();
    run.ok(&["qa", "contents-scroll", "60"]);
    run.ok(&["qa", "scroll", "42"]);
    run.settled();
    // Side by side with the second, synced, each pane in its own place, and the third still the one read (owner, 2026-11-08: comparison view).
    run.ok(&["compare", "--tabs", "#2,#3", "--sync", "on"]);
    run.settled();
    run.ok(&["qa", "pane-scroll", "0:120"]);
    run.settled();
    run.ok(&["qa", "pane-focus", "1"]);
    run.settled();
    // Everything else there is.
    run.ok(&["qa", "resize", "1300x820"]);
    run.ok(&["qa", "sidebar-width", "318"]);
    run.ok(&["qa", "panel", "toggle"]);
    run.ok(&["view", "--paper", "warm", "--agent-icon", "off"]);
    run.ok(&["settings", "--auto-update", "off"]);
    run.ok(&["qa", "settings", "open"]);
    run.ok(&["qa", "full-screen", "on"]);
    run.ok(&["qa", "find", "open"]);
    run.ok(&["qa", "find-query", "ran this"]);
    run.ok(&["qa", "find-draft", "typed but not run"]);
    run.ok(&["qa", "focus", "find"]);
    run.ok(&["qa", "frontmost", "off"]);
    run.ok(&["qa", "edit-mode", "on"]);
    run.ok(&["qa", "view-rotation", "1"]);
    run.settled();
}

/// Failure: anything the user can see or has changed doesn't survive a quit and a launch. **The test that matters most:** the whole of `qa state` is compared, across a normal quit, with a short named list of exceptions. A field added to `qa state` fails this until it's carried over or named, so state is complete by construction. In dark too, with light and dark screenshots to look at when `RECTO_RESTORE_SHOTS` names a folder. And going round twice loses nothing the first time didn't (what `quitting_twice_in_a_row_changes_nothing` held with a rich state of its own): in light, a record written from a restored session is the one before it, a second quit and launch later.
#[test]
fn everything_comes_back_after_a_quit() {
    comes_back_after_a_quit([&[], &[]], true);
}

/// Failure (CI, main at faa9180: "⌘F gives the field the keyboard"): ⌘F and typing in the moment a window was up without the state, read late, were lost when the state came in and the window was made again from it, the new one with the page focused. **Acting within that gap,** the state read 1.5 s late and the window made again from it a second later: what's done waits for the window made from the state, and all of it is there after: Find open with what was typed, and its field with the keyboard.
#[test]
fn what_is_done_before_a_late_state_comes_in_isnt_lost() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let file = long_document("alpha", 20);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "9"]);
    run.settled();
    run.quit();
    // The state 1.5 s late, and the window made again from it a second after that: commands are taken once the state's in, so these come in that second, as on CI they did in a moment of it.
    let run = Run::on_with_env(
        &state,
        &[
            ("RECTO_QA_RESTORE_DELAY_MS", std::ffi::OsStr::new("1500")),
            ("RECTO_QA_REMAKE_DELAY_MS", std::ffi::OsStr::new("1000")),
        ],
    )
    .unwrap();
    run.ok(&["qa", "menu", "find.open"]);
    run.ok(&["qa", "find-type", "words"]);
    run.settled();
    let after = run.report();
    assert_eq!(after["session"]["tabs"].as_array().map(Vec::len), Some(1), "the state's come in");
    assert_eq!(after["app"]["find"]["open"], true, "{}", after["app"]["find"]);
    assert_eq!(
        after["app"]["find"]["field_has_focus"], true,
        "⌘F gave the field the keyboard: {}",
        after["app"]["find"]
    );
    assert_eq!(after["app"]["focus_view"], "find");
    assert_eq!(after["app"]["find"]["untrusted"]["field"], "words", "what was typed: {}", after["app"]["find"]);
    assert_eq!(after["session"]["tabs"][0]["page"], 8, "and the place from the state");
    run.quit();
}

/// **The real app's test of a window made again from a state that came in late,** for the slow stage's tests retired for headless twins (R1.4): it says, on each
/// launch, that the window did go up without the state and was made again (`record.remade`), so it can't pass on the on-time path.
///
/// Failure (CI, main at f4b4d22): on a slower machine the state was read after the window's second of patience, the window went up without it, and what came in later didn't bring everything back: the sidebar's contents with no height there, and here, with the disk slow on purpose, the window's size and full screen, the appearance, Settings, every pane's place and the contents' scroll. Just past the patience, the window that went up without the state also wrote its own default size over what had come in. **The whole of `qa state` across a quit, with the state read 1.05 s and 1.5 s late**, so it comes in after the window's up. Everything comes back as from an on-time read.
#[test]
fn everything_comes_back_when_the_state_is_read_late() {
    // Just past the patience, where the window that went up without it can still be saying its own size and focus as the state comes in; and well past it.
    comes_back_after_a_quit(
        [
            &[("RECTO_QA_RESTORE_DELAY_MS", std::ffi::OsStr::new("1050"))],
            &[("RECTO_QA_RESTORE_DELAY_MS", std::ffi::OsStr::new("1500"))],
        ],
        false,
    );
}

/// A rich state, a quit, and a launch with this in its environment, and the whole of `qa state` compared, in light and in dark; with `twice`, a second quit and launch in light, compared with the first.
fn comes_back_after_a_quit(second_launch: [&[(&str, &std::ffi::OsStr)]; 2], twice: bool) {
    for ((look, paper), second_launch) in [("light", "warm"), ("dark", "dark")].into_iter().zip(second_launch) {
        let state = State::new();
        let Some(run) = Run::on(&state) else { return };
        let (a, b, c) = (long_document("alpha", 60), long_document("beta", 30), chaptered("gamma", 20, 8));
        run.ok(&["qa", "scroller-style", "overlay"]);
        run.ok(&["qa", "appearance", look]);
        make_rich(&run, &[&a, &b, &c]);
        run.ok(&["view", "--paper", paper]);
        run.settled();
        let shots = std::env::var_os("RECTO_RESTORE_SHOTS").map(PathBuf::from);
        if let Some(folder) = &shots {
            let _ = std::fs::remove_file(folder.join(format!("before-{look}.png")));
            run.ok(&["qa", "shot", "--out", folder.join(format!("before-{look}.png")).to_str().unwrap()]);
        }
        let before = run.report();
        run.quit();
        let run = Run::on_with_env(&state, second_launch).unwrap();
        // Read late on purpose, it's waited for: what's compared is what's there once it has been.
        let tabs = before["session"]["tabs"].as_array().unwrap().len();
        common::app::wait_for_long(&run, |report| report["session"]["tabs"].as_array().is_some_and(|now| now.len() == tabs));
        run.ok(&["qa", "scroller-style", "overlay"]);
        run.settled();
        let after = run.report();
        if let Some(folder) = &shots {
            let _ = std::fs::remove_file(folder.join(format!("after-{look}.png")));
            run.ok(&["qa", "shot", "--out", folder.join(format!("after-{look}.png")).to_str().unwrap()]);
        }
        assert_same(&before, &after, &known_paths());
        // The exact pixel, at the same zoom, on the first frame.
        for field in ["scroll_y", "scroll_x", "magnification"] {
            let (was, now) = (before["app"]["pages"][field].as_f64().unwrap(), after["app"]["pages"][field].as_f64().unwrap());
            assert!((was - now).abs() < 0.25, "{field}: {was} then {now}");
        }
        assert_eq!(after["session"]["find"]["draft"], "typed but not run");
        assert_eq!(after["session"]["app"]["focus"]["view"], "find");
        assert_eq!(
            after["app"]["find"]["field_has_focus"], true,
            "Find's field has the keyboard: {}",
            after["app"]["find"]
        );
        // Read late, the window went up without the state and was made again from it: what's compared above is Swift's window made from a late state, and
        // not an on-time launch on a quick machine.
        let late = !second_launch.is_empty();
        assert_eq!(after["record"]["remade"], late, "{look}: went up without the state and made again from it");
        // Round again: what a restored session writes is what it was restored from.
        let run = if twice && look == "light" {
            run.quit();
            let run = Run::on(&state).unwrap();
            run.ok(&["qa", "scroller-style", "overlay"]);
            run.settled();
            assert_same(&after, &run.report(), &[]);
            run
        } else {
            run
        };
        // It's all still there to use: undo and redo with whose each is.
        run.ok(&["qa", "owner-edit", "redo"]);
        run.quit();
    }
}

/// Failure: Find doesn't come back as it was left (decision 23): its field, the hits of what was found, the hit gone to, and that hit selected on the page, after a quit and a launch. The hits are found again, as they're never kept, and the hit is chosen again by its place.
#[test]
fn find_comes_back_with_its_hits_and_the_hit_gone_to() {
    find_comes_back(&[]);
}

/// Failure (slow mode, 2026-11-11: with the control socket's first answer late, Find came back with its hits and not the hit gone to): the app heard of Find's hits only once the socket's server ran, so a search that finished before then was heard by nobody, and the hit was never chosen again. The same with the socket another session's, which a launch allows, for every search. The socket answering 2 s late on the launch after the quit, and the hit comes back.
#[test]
fn find_comes_back_when_the_socket_answers_late() {
    find_comes_back(&[("RECTO_QA_SLOW", std::ffi::OsStr::new("socket=2000"))]);
}

/// Find left on its third hit, a quit, and a launch with this in its environment: all of it comes back.
fn find_comes_back(second_launch: &[(&str, &std::ffi::OsStr)]) {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let file = long_document("findable", 30);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "find", "open"]);
    run.ok(&["qa", "find-type", "findable page 1"]);
    run.settled();
    for _ in 0..3 {
        run.ok(&["qa", "find-key", "return"]);
    }
    run.settled();
    let before = run.report();
    let find = &before["app"]["find"];
    // `findable page 1` on page 2, and `page 10` to `page 19` on pages 11 to 20: eleven, a page row and a hit each.
    assert_eq!(
        (find["rows"].as_u64(), find["status"].as_str()),
        (Some(22), Some("11 matches on 11 pages")),
        "{find}"
    );
    assert_eq!(find["current_row"], 5, "the third hit's row: {find}");
    assert_eq!(before["session"]["find"]["current"], json!([11, 0]), "{}", before["session"]["find"]);
    assert_eq!(before["app"]["place"], "12 of 30");
    assert!(
        before["app"]["pages"]["highlight_rects"].as_u64().unwrap_or(0) > 0,
        "the hit is selected on the page: {}",
        before["app"]["pages"]
    );
    run.quit();
    let run = Run::on_with_env(&state, second_launch).unwrap();
    run.settled();
    let after = run.report();
    for field in ["open", "rows", "status", "current_row", "untrusted", "list_height", "rows_on_screen"] {
        assert_eq!(after["app"]["find"][field], before["app"]["find"][field], "{field}: {}", after["app"]["find"]);
    }
    assert_eq!(after["app"]["place"], before["app"]["place"]);
    assert_eq!(
        after["app"]["pages"]["highlight_rects"], before["app"]["pages"]["highlight_rects"],
        "the hit's still selected"
    );
    // And it goes on from there.
    run.ok(&["qa", "find-key", "return"]);
    run.settled();
    assert_eq!(run.report()["app"]["find"]["current_row"], 7);
    run.quit();
    // The file replaced while the app was away, with more pages: what's found is the new file's, not the old count or the old places (review, round 1).
    let longer: Vec<Page> = (0..120)
        .map(|index| Page::new(text_lines(&[&format!("findable page {index}"), "Words, so that the page isn't blank"])))
        .collect();
    std::fs::write(&file.0, pages(&longer)).unwrap();
    let run = Run::on(&state).unwrap();
    run.settled();
    let replaced = run.report();
    // `page 1` on page 2, `page 10`-`19` and `page 100`-`119`.
    assert_eq!(replaced["app"]["find"]["status"], "31 matches on 31 pages", "{}", replaced["app"]["find"]);
    run.quit();
}

/// Failure: the first frame shows the wrong page or the home screen, or the window comes up empty and fills in. It's held back until the page is on its layers, and this is what was on it when it showed: the place, drawn.
#[test]
fn the_first_frame_is_the_right_one() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let file = long_document("first", 60);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "33"]);
    run.settled();
    run.ok(&["qa", "scroll", "71"]);
    run.settled();
    let before = run.report();
    run.quit();
    let run = Run::on(&state).unwrap();
    run.settled();
    let shown = run.report()["record"]["first_shown"].clone();
    assert_eq!(shown["home"], false, "{shown}");
    // Something on the page, from its sleep file if nothing else: a window doesn't wait for sharp tiles, which on a heavy page is most of a second.
    assert!(shown["tile_layers"].as_u64().unwrap() > 0, "{shown}");
    assert_eq!(shown["page"], before["reader"]["page"]);
    assert!(
        (shown["scroll_y"].as_f64().unwrap() - before["reader"]["scroll_y"].as_f64().unwrap()).abs() < 0.25,
        "{shown} {}",
        before["reader"]
    );
}

/// Failure: an update mid-work loses what the reader had going, or asks them something. It hands off in the middle of Find text typed and edit mode on, and the whole report comes back. It never prompts: there's nothing to answer in a quiet run, and it ends by itself.
#[test]
fn a_hand_off_in_the_middle_of_things_loses_nothing() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let (a, b, c) = (long_document("alpha", 60), long_document("beta", 30), chaptered("gamma", 20, 8));
    run.ok(&["qa", "scroller-style", "overlay"]);
    make_rich(&run, &[&a, &b, &c]);
    let before = run.report();
    let flushed = run.hand_off();
    assert_eq!(flushed["handed_off"], true);
    assert_eq!(flushed["edited_tabs"], 1, "{flushed}");
    let run = Run::on(&state).unwrap();
    run.ok(&["qa", "scroller-style", "overlay"]);
    run.settled();
    let after = run.report();
    assert_same(&before, &after, &known_paths());
    assert_eq!(after["session"]["edit_mode"], true);
    assert_eq!(after["session"]["find"]["draft"], "typed but not run");
}

/// Failure: an update goes ahead with a sheet open or an agent's command running, so it's lost. The check and the hand-off both say what's in the way, and it can go once that's over. How long since the user touched the app is one number, there to ask, with what touches it resetting it.
#[test]
fn a_hand_off_waits_for_what_is_in_flight() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let file = long_document("busy", 5);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    assert_eq!(run.ok(&["session", "check"])["safe"], true);
    run.ok(&["qa", "sheet", "open"]);
    let check = run.ok(&["session", "check"]);
    assert_eq!(check["safe"], false);
    assert_eq!(check["busy"][0]["code"], "sheet_open");
    let (reply, code) = state.json(&["qa", "handoff", "now"]);
    assert_ne!(code, 0, "{reply}");
    assert!(reply["error"]["message"].as_str().unwrap().contains("sheet"), "{reply}");
    assert!(process_counters(run.pid).is_some(), "it's still here");
    run.ok(&["qa", "sheet", "closed"]);
    std::thread::sleep(Duration::from_millis(150));
    let idle = run.ok(&["session", "check"])["seconds_since_input"].as_f64().unwrap();
    assert!(idle >= 0.1, "{idle}");
    run.ok(&["qa", "note-input", "now"]);
    assert!(run.ok(&["session", "check"])["seconds_since_input"].as_f64().unwrap() < idle);
    run.hand_off();
}

/// Failure: a tab that goes cold during a session loses its zoom and where it was in the page, as a cold tab holds nothing in the page view, and comes back at the top of the page at no zoom. It's the same fault a restart would have, and the session keeps both now.
#[test]
fn a_tab_gone_cold_comes_back_where_it_was() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let files: Vec<TempPdf> = (0..5).map(|at| long_document(&format!("cold{at}"), 20)).collect();
    run.ok(&["open", files[0].0.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "9"]);
    run.settled();
    run.ok(&["qa", "zoom", "1.5"]);
    run.settled();
    run.ok(&["qa", "scroll", "90"]);
    run.settled();
    let was = run.report();
    for file in &files[1..] {
        run.ok(&["open", file.0.to_str().unwrap()]);
        run.settled();
    }
    assert!(run.report()["reader"]["documents_held"].as_u64().unwrap() <= 3);
    run.ok(&["tap", "#1"]);
    run.settled();
    let back = run.report();
    for field in ["scroll_y", "scroll_x", "magnification"] {
        let (one, two) = (was["app"]["pages"][field].as_f64().unwrap(), back["app"]["pages"][field].as_f64().unwrap());
        assert!((one - two).abs() < 0.25, "{field}: {one} then {two}");
    }
    assert_eq!(back["reader"]["page"], was["reader"]["page"]);
}

/// Waits until what's been edited has gone to its journal, which takes seconds, and says what the lists are.
fn sleep_files(state: &State) -> Vec<PathBuf> {
    std::fs::read_dir(state.root.join("session").join("journals"))
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("sleep-"))
        .map(|entry| entry.path())
        .collect()
}

fn text_document(folder: &Path, name: &str, count: usize) -> PathBuf {
    let path = folder.join(name);
    std::fs::write(
        &path,
        pages(&(0..count).map(|at| Page::new(text_lines(&[&format!("{name} page {at}")]))).collect::<Vec<_>>()),
    )
    .unwrap();
    path
}

/// Failure (decision 25, in the real app): a tab left alone stays in memory, or goes with its edits lost, or wakes bare, at the wrong place, or by reading its PDF. It goes dormant on the one deadline: no worker, no view, no list in memory, and a sleep file. Tapped, it's up from that file at its place, with a picture on every page before anything's drawn, then sharp, edits and history there.
#[test]
fn a_tab_left_alone_goes_dormant_and_wakes_from_its_file() {
    let state = State::at(Level::Full);
    let Some(run) = Run::on(&state) else { return };
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let (left, read) = (text_document(&folder, "left.pdf", 12), text_document(&folder, "read.pdf", 3));
    run.ok(&["open", left.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "6"]);
    run.settled();
    run.ok(&["qa", "owner-edit", "rotate:1"]);
    run.settled();
    let place = |run: &Run<'_>| run.report()["reader"]["scroll_y"].as_f64().unwrap();
    let at_page_6 = place(&run);
    run.ok(&["open", read.to_str().unwrap()]);
    run.settled();
    let tabs = tabs_of(&run.report());
    let left_tab = tabs.iter().find(|tab| tab["path"].as_str().unwrap().ends_with("left.pdf")).unwrap()["id"]
        .as_u64()
        .unwrap();
    assert_eq!(run.report()["reader"]["documents_held"], 2, "it's warm for now");
    let workers_before = run.report()["reader_workers"].clone();
    // Left alone, a second counting as five minutes.
    run.ok(&["qa", "dormant-after", "1"]);
    // Its sleep file's written off the session thread, by the queue's writer, which ends once it's done.
    wait_until("it goes dormant", || {
        let report = run.report();
        report["record"]["sleep"]["gone_dormant"]["left_alone"].as_u64().unwrap() >= 1
            && report["reader"]["documents_held"] == 1
            && report["record"]["sleep"]["leaving"]["writer"] == false
    });
    let after = run.report();
    assert_eq!(
        after["session"]["edits_full"].as_object().unwrap().len(),
        1,
        "its edits are still there, read from disk for this: {workers_before}"
    );
    assert_eq!(sleep_files(&state).len(), 1, "with a sleep file");
    assert!(tabs_of(&after).iter().any(|tab| tab["id"] == left_tab), "and the tab's there");
    // Nothing more's written with nothing happening, and there's nothing left to come due but what's on hand.
    let wrote = after["record"]["wrote_bytes"].as_u64().unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(run.report()["record"]["wrote_bytes"].as_u64().unwrap(), wrote, "no more to write");
    run.ok(&["qa", "dormant-after", "300"]);
    // Tapped.
    run.ok(&["tap", &format!("#{left_tab}")]);
    run.settled();
    let woken = run.report();
    let wake = &woken["reader"]["last_wake"];
    assert_eq!(wake["from_sleep"], true, "{woken}");
    assert_eq!(wake["blank_frames"], 0, "{wake}");
    assert!(wake["first_frame_us"].as_u64().is_some(), "{wake}");
    assert_eq!(run.ok(&["session", "show"])["tabs"][0]["can_undo"], true);
    assert!(wake["sharp_us"].as_u64().unwrap() >= wake["first_frame_us"].as_u64().unwrap(), "{wake}");
    assert!((place(&run) - at_page_6).abs() < 1.0, "at its place: {} and {at_page_6}", place(&run));
    assert_eq!(run.ok(&["edits"])["edited"], true);
    assert_eq!(run.ok(&["edits"])["can_undo"], true);
    assert!(run.report()["record"]["sleep"]["woken_from_file"].as_u64().unwrap() >= 1);
    run.ok(&["undo"]);
    assert_eq!(run.ok(&["edits"])["edited"], false, "and its history works");
    run.ok(&["redo"]);
}

/// Failure (every crash point of going dormant): a list that's gone to disk is lost or changed by a kill, or a hand-off's read-back fails on it, or says it's fine when its journal isn't there. It's the journal that holds it, so each comes back exactly, and a hand-off with one asleep goes through and comes back the same.
#[test]
fn a_dormant_tabs_edits_survive_a_kill_and_a_hand_off() {
    let state = State::at(Level::Full);
    let Some(run) = Run::on(&state) else { return };
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let (asleep, showing) = (text_document(&folder, "asleep.pdf", 8), text_document(&folder, "showing.pdf", 3));
    run.ok(&["open", asleep.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "owner-edit", "delete:2"]);
    run.ok(&["qa", "owner-edit", "rotate:1"]);
    run.ok(&["qa", "owner-edit", "undo"]);
    run.settled();
    run.ok(&["open", showing.to_str().unwrap()]);
    run.settled();
    let tab = tabs_of(&run.report())
        .iter()
        .find(|tab| tab["path"].as_str().unwrap().ends_with("asleep.pdf"))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let lists = run.report()["session"]["edits_full"].clone();
    let slept = run.ok(&["session", "sleep", "--tab", &format!("#{tab}")]);
    assert_eq!((&slept["dormant"], &slept["list_asleep"]), (&json!(true), &json!(true)), "{slept}");
    assert_eq!(run.report()["session"]["edits_full"], lists, "read from its journal, it's what was in memory");
    // A kill, with it asleep.
    run.kill();
    let run = Run::on(&state).unwrap();
    run.settled();
    assert_eq!(run.report()["session"]["edits_full"], lists, "after a kill");
    // A hand-off, with it asleep, as a launch has left it.
    let summary = tabs_of(&run.report()).iter().find(|tab_now| tab_now["id"] == tab).unwrap()["edits"].clone();
    let handed = run.hand_off();
    assert!(handed.get("error").is_none(), "{handed}");
    let run = Run::on(&state).unwrap();
    run.settled();
    assert_eq!(run.report()["session"]["edits_full"], lists, "after a hand-off");
    assert_eq!(tabs_of(&run.report()).iter().find(|tab_now| tab_now["id"] == tab).unwrap()["edits"], summary);
    // Woken, it does what it did.
    run.ok(&["tap", &format!("#{tab}")]);
    run.settled();
    assert_eq!(run.ok(&["edits"])["pages"], 7);
    assert_eq!(run.ok(&["edits"])["can_redo"], true);
    run.ok(&["undo"]);
    assert_eq!(run.ok(&["edits"])["pages"], 8);
}

/// A page of one colour, for telling which file a picture's of. The red, green and blue of each are as far apart as can be.
fn page_of(colour: &str) -> Page {
    Page::new(format!("{colour} rg 0 0 612 792 re f").into_bytes())
}

const RED: &str = "1 0 0";
const BLUE: &str = "0 0 1";
const GREEN: &str = "0 1 0";

/// A PDF of pages of one colour with a file ID, as anything that writes PDFs gives one: what a version is told by when a stamp can't.
#[allow(clippy::disallowed_methods)] // A test fixture built with lopdf's eager writer, which alone encrypts or rebuilds; never over a user's file.
fn coloured_with_id(colours: &[&str], id: &str) -> Vec<u8> {
    let mut document = lopdf::Document::load_mem(&pages(&colours.iter().map(|colour| page_of(colour)).collect::<Vec<_>>())).unwrap();
    document.trailer.set(
        "ID",
        lopdf::Object::Array(vec![lopdf::Object::string_literal(id), lopdf::Object::string_literal(id)]),
    );
    let mut out = Vec::new();
    document.save_to(&mut out).unwrap();
    out
}

/// Which of red, green and blue each sleep file's pictures mostly are, read as anything else would, by the file's name.
fn colours_in_sleep_files(state: &State) -> Vec<(String, Vec<&'static str>)> {
    sleep_files(state)
        .iter()
        .map(|file| {
            let sleep = recto_session::sleep::decode_sleep(&std::fs::read(file).unwrap(), recto_session::record::CURRENT_VERSION).unwrap();
            let colours = sleep.pictures.iter().map(|picture| {
                let (blue, green, red) = picture.bgra.as_chunks::<4>().0.iter().fold((0u64, 0u64, 0u64), |sum, pixel| {
                    (sum.0 + u64::from(pixel[0]), sum.1 + u64::from(pixel[1]), sum.2 + u64::from(pixel[2]))
                });
                // Black on white paper, as another tab's is, is none of them.
                if red > green.max(blue) {
                    "red"
                } else if green > red.max(blue) {
                    "green"
                } else if blue > red.max(green) {
                    "blue"
                } else {
                    "paper"
                }
            });
            (file.file_name().unwrap().to_string_lossy().to_string(), colours.collect())
        })
        .collect()
}

/// Failure (full check, 2026-11-08, reproduced with 3 GB memory spikes beside the run): a tab left for another has what it showed marked discardable, and the system takes that back whenever it needs the pages, with no word to the app. Put to sleep after that, the tab's sleep file had no picture at all (`left: []` in `a_new_version_of_a_file_has_none_of_the_old_ones_pictures`), and it woke to bare paper. It sleeps with what it showed when it was left.
#[test]
fn a_tab_put_to_sleep_has_its_picture_after_the_system_takes_its_tiles() {
    let state = State::at(Level::Full);
    let Some(run) = Run::on(&state) else { return };
    run.ok(&["qa", "appearance", "light"]);
    run.ok(&["qa", "sleep-write-gap", "0"]);
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let (file, other) = (folder.join("colour.pdf"), text_document(&folder, "other.pdf", 2));
    std::fs::write(&file, pages(&[page_of(RED)])).unwrap();
    run.ok(&["open", file.to_str().unwrap()]);
    run.settled();
    run.ok(&["open", other.to_str().unwrap()]);
    run.settled();
    let on_tab = tab_on(&run, "colour.pdf");
    run.ok(&["qa", "memory-pressure", "discard"]);
    assert_eq!(run.ok(&["session", "sleep", "--tab", &on_tab])["dormant"], true);
    assert_eq!(
        colours_in_sleep_files(&state)
            .iter()
            .flat_map(|(_, colours)| colours.clone())
            .collect::<Vec<_>>(),
        ["red"],
        "what it showed when it was left"
    );
}

/// Failure (review, blocks the merge): a sleep file keeps the old file's picture under the new file's name, for good: a red PDF replaced by a blue one, or saved over, has red in the next sleep file and every wake after it, restarts included. With a redaction, that picture would be what was removed. Everything drawn of a file goes with that version of it, on this route and on a save.
#[test]
fn a_new_version_of_a_file_has_none_of_the_old_ones_pictures() {
    let state = State::at(Level::Full);
    let Some(run) = Run::on(&state) else { return };
    run.ok(&["qa", "appearance", "light"]);
    run.ok(&["qa", "sleep-write-gap", "0"]);
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let (file, other) = (folder.join("colour.pdf"), text_document(&folder, "other.pdf", 2));
    std::fs::write(&file, pages(&[page_of(RED)])).unwrap();
    run.ok(&["open", file.to_str().unwrap()]);
    run.settled();
    run.ok(&["open", other.to_str().unwrap()]);
    run.settled();
    let tab = tabs_of(&run.report())
        .iter()
        .find(|tab| tab["path"].as_str().unwrap().ends_with("colour.pdf"))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let on_tab = format!("#{tab}");
    let other_tab = format!(
        "#{}",
        tabs_of(&run.report())
            .iter()
            .find(|tab| tab["path"].as_str().unwrap().ends_with("other.pdf"))
            .unwrap()["id"]
            .as_u64()
            .unwrap()
    );
    run.ok(&["session", "sleep", "--tab", &on_tab]);
    assert_eq!(
        colours_in_sleep_files(&state)
            .iter()
            .flat_map(|(_, colours)| colours.clone())
            .collect::<Vec<_>>(),
        ["red"],
        "the old one, red, as it should be"
    );
    // Another app puts blue there, and it's woken from what it had.
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&file, pages(&[page_of(BLUE), page_of(BLUE)])).unwrap();
    run.ok(&["tap", &on_tab]);
    wait_until("it finds the new one", || run.ok(&["where"])["pages"] == 2);
    run.settled();
    assert_eq!(tabs_of(&run.report()).iter().find(|one| one["id"] == tab).unwrap()["notice"], "changed");
    run.ok(&["tap", &other_tab]);
    run.settled();
    run.ok(&["session", "sleep", "--tab", &on_tab]);
    let colours = colours_in_sleep_files(&state);
    assert!(
        !colours.iter().any(|(_, colours)| colours.contains(&"red")),
        "nothing red, under any name: {colours:?}"
    );
    assert!(
        colours.iter().any(|(_, colours)| colours.contains(&"blue")),
        "and it's the new one that's there: {colours:?}"
    );
    // A save, with no sleep file in between to hide behind: the picture's still in memory at the moment it's saved.
    run.ok(&["tap", &on_tab]);
    run.settled();
    let written = folder.join("written.pdf");
    std::fs::write(&written, pages(&[page_of(GREEN), page_of(GREEN)])).unwrap();
    run.ok(&["qa", "save-from", written.to_str().unwrap()]);
    run.settled();
    run.ok(&["tap", &other_tab]);
    run.settled();
    run.ok(&["session", "sleep", "--tab", &on_tab]);
    wait_until("the old version's goes", || {
        !colours_in_sleep_files(&state).iter().any(|(_, colours)| colours.contains(&"blue"))
    });
    assert!(
        colours_in_sleep_files(&state).iter().any(|(_, colours)| colours.contains(&"green")),
        "{:?}",
        colours_in_sleep_files(&state)
    );
    // And across a restart, what comes up isn't any of the old ones, and its next file isn't either.
    run.quit();
    let run = Run::on(&state).unwrap();
    run.ok(&["qa", "appearance", "light"]);
    run.settled();
    run.ok(&["tap", &on_tab]);
    run.settled();
    assert_eq!(run.ok(&["where"])["pages"], 2);
    run.ok(&["tap", &other_tab]);
    run.settled();
    run.ok(&["session", "sleep", "--tab", &on_tab]);
    let colours = colours_in_sleep_files(&state);
    assert!(
        !colours.iter().any(|(_, colours)| colours.contains(&"red") || colours.contains(&"blue")),
        "{colours:?}"
    );
}

fn tab_on(run: &Run<'_>, name: &str) -> String {
    format!(
        "#{}",
        tabs_of(&run.report()).iter().find(|tab| tab["path"].as_str().unwrap().ends_with(name)).unwrap()["id"]
    )
}

/// Failure (review follow-up 1, the red frames, reproduced here before the fix): a warm tab's file is replaced, by a save or another app, and tapping it shows the old pages from what's held and from a worker that still has the old file open, with nothing said. Its file's looked at before it's shown, and the old pages go.
#[test]
fn a_warm_tab_shows_its_file_as_it_is_now() {
    let state = State::at(Level::Full);
    let Some(run) = Run::on(&state) else { return };
    run.ok(&["qa", "appearance", "light"]);
    run.ok(&["qa", "sleep-write-gap", "0"]);
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let (file, other) = (folder.join("colour.pdf"), text_document(&folder, "other.pdf", 2));
    std::fs::write(&file, pages(&[page_of(RED), page_of(RED)])).unwrap();
    run.ok(&["open", file.to_str().unwrap()]);
    run.settled();
    run.ok(&["open", other.to_str().unwrap()]);
    run.settled();
    assert_eq!(run.report()["reader"]["documents_held"], 2, "it's warm, with its pages held");
    let replacement = folder.join("replacement.pdf");
    std::fs::write(&replacement, pages(&[page_of(BLUE), page_of(BLUE)])).unwrap();
    std::fs::rename(&replacement, &file).unwrap();
    run.ok(&["tap", &tab_on(&run, "colour.pdf")]);
    run.settled();
    assert_eq!(
        tabs_of(&run.report())
            .iter()
            .find(|tab| tab["path"].as_str().unwrap().ends_with("colour.pdf"))
            .unwrap()["notice"],
        "changed"
    );
    run.ok(&["tap", &tab_on(&run, "other.pdf")]);
    run.settled();
    run.ok(&["session", "sleep", "--tab", &tab_on(&run, "colour.pdf")]);
    let colours = colours_in_sleep_files(&state);
    assert!(
        colours.iter().any(|(_, colours)| colours.contains(&"blue")) && !colours.iter().any(|(_, colours)| colours.contains(&"red")),
        "{colours:?}"
    );
}

/// Failure (review follow-up 1): a file replaced while the app was closed, and its tab the one on screen: the window goes up with the old file's picture from its sleep file, before anything's looked at the file.
#[test]
fn a_launch_doesnt_show_a_picture_of_a_file_thats_been_replaced() {
    let state = State::at(Level::Full);
    let Some(run) = Run::on(&state) else { return };
    run.ok(&["qa", "appearance", "light"]);
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("colour.pdf");
    std::fs::write(&file, pages(&[page_of(RED)])).unwrap();
    run.ok(&["open", file.to_str().unwrap()]);
    run.settled();
    run.hand_off();
    assert!(
        colours_in_sleep_files(&state).iter().any(|(_, colours)| colours.contains(&"red")),
        "there's a red picture to come up with"
    );
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&file, pages(&[page_of(BLUE), page_of(BLUE)])).unwrap();
    let run = Run::on(&state).unwrap();
    run.settled();
    let report = run.report();
    assert_eq!(
        report["reader"]["last_wake"]["from_sleep"], false,
        "no picture of the old one: {}",
        report["reader"]["last_wake"]
    );
    assert_eq!(run.ok(&["where"])["pages"], 2);
}

/// Failure (review follow-up 3): a file rewritten with its size, time and number as they were keeps the old picture in its next sleep file, as nothing but the stamp was compared. Which version of a file it is comes from what opening it reads anyway, its ID and where its trailers end, and a picture of another goes. A file with no ID whose every object's at the same place can't be told this way: see `docs/engineering/state-record.md`.
#[test]
fn a_rewrite_that_keeps_its_stamp_doesnt_keep_its_picture() {
    let state = State::at(Level::Full);
    let Some(run) = Run::on(&state) else { return };
    run.ok(&["qa", "appearance", "light"]);
    run.ok(&["qa", "sleep-write-gap", "0"]);
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let (file, other) = (folder.join("colour.pdf"), text_document(&folder, "other.pdf", 2));
    std::fs::write(&file, coloured_with_id(&[RED], "first-version-id")).unwrap();
    let stamp = recto_platform::file_stamp(&file).unwrap();
    run.ok(&["open", file.to_str().unwrap()]);
    run.settled();
    run.ok(&["open", other.to_str().unwrap()]);
    run.settled();
    run.ok(&["session", "sleep", "--tab", &tab_on(&run, "colour.pdf")]);
    assert!(colours_in_sleep_files(&state).iter().any(|(_, colours)| colours.contains(&"red")));
    // The same length, the same file, and its time put back.
    let modified = std::fs::metadata(&file).unwrap().modified().unwrap();
    std::fs::write(&file, coloured_with_id(&[BLUE], "other-version-id")).unwrap();
    std::fs::File::options().write(true).open(&file).unwrap().set_modified(modified).unwrap();
    assert_eq!(recto_platform::file_stamp(&file).unwrap(), stamp, "no stamp can tell");
    run.ok(&["tap", &tab_on(&run, "colour.pdf")]);
    run.settled();
    run.ok(&["tap", &tab_on(&run, "other.pdf")]);
    run.settled();
    run.ok(&["session", "sleep", "--tab", &tab_on(&run, "colour.pdf")]);
    let colours = colours_in_sleep_files(&state);
    assert!(!colours.iter().any(|(_, colours)| colours.contains(&"red")), "{colours:?}");
}

/// Failure (review follow-up 2): `session sleep` writes a new sleep file every time it's asked, so an agent going tap, move, sleep writes most of a megabyte in ten turns. It keeps to the same limits as anything else that isn't a quit or a hand-off.
#[test]
fn session_sleep_keeps_to_the_limits_on_writing() {
    let state = State::at(Level::Full);
    let Some(run) = Run::on(&state) else { return };
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let (file, other) = (text_document(&folder, "doc.pdf", 12), text_document(&folder, "other.pdf", 2));
    run.ok(&["open", file.to_str().unwrap()]);
    run.settled();
    run.ok(&["open", other.to_str().unwrap()]);
    run.settled();
    for turn in 0..5 {
        run.ok(&["tap", &tab_on(&run, "doc.pdf")]);
        run.settled();
        run.ok(&["goto", &format!("{}", turn * 2 + 2)]);
        run.settled();
        run.ok(&["tap", &tab_on(&run, "other.pdf")]);
        run.settled();
        run.ok(&["session", "sleep", "--tab", &tab_on(&run, "doc.pdf")]);
    }
    let sleep = run.report()["record"]["sleep"].clone();
    assert_eq!(sleep["sleep_files_written"], 1, "one, the first: {sleep}");
    assert!(sleep["writes_put_off"].as_u64().unwrap() >= 4, "{sleep}");
}

/// Failure (review follow-up 4): with a tab's edits unreadable just now, `recto text` says there's no such page, for every page, which says nothing of why. It says its edits can't be read.
#[test]
fn reading_a_page_says_when_its_edits_cant_be_read() {
    let state = State::at(Level::Full);
    let Some(run) = Run::on(&state) else { return };
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let (file, other) = (text_document(&folder, "doc.pdf", 4), text_document(&folder, "other.pdf", 2));
    run.ok(&["open", file.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "owner-edit", "delete:2"]);
    run.settled();
    run.ok(&["open", other.to_str().unwrap()]);
    run.settled();
    let slept = run.ok(&["session", "sleep", "--tab", &tab_on(&run, "doc.pdf")]);
    assert_eq!(slept["list_asleep"], true);
    let journal = std::fs::read_dir(state.root.join("session").join("journals"))
        .unwrap()
        .flatten()
        .find(|entry| entry.file_name().to_string_lossy().starts_with("journal-"))
        .unwrap()
        .path();
    std::fs::write(&journal, b"not a journal").unwrap();
    let (reply, _) = state.json(&["text", "--tab", &tab_on(&run, "doc.pdf"), "--page", "1"]);
    assert_eq!(reply["error"]["code"], "edits_unreadable", "{reply}");
}

/// Failure (review, blocks the merge): a protected PDF's pages go in a sleep file in the clear, when it's been added to a plain document: only the tab's own file was looked at. A document with a page from one has no pictures, and that's known after a restart.
#[test]
#[allow(clippy::disallowed_methods)] // A test fixture built with lopdf's eager writer, which alone encrypts or rebuilds; never over a user's file.
fn a_protected_pdfs_pages_never_reach_a_sleep_file() {
    use lopdf::encryption::{EncryptionState, EncryptionVersion, Permissions};
    let state = State::at(Level::Full);
    let Some(run) = Run::on(&state) else { return };
    run.ok(&["qa", "appearance", "light"]);
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let plain = folder.join("plain.pdf");
    std::fs::write(&plain, pages(&[page_of(GREEN), page_of(GREEN)])).unwrap();
    let other = text_document(&folder, "other.pdf", 2);
    let mut document = lopdf::Document::load_mem(&pages(&[page_of(RED)])).unwrap();
    document.trailer.set(
        "ID",
        lopdf::Object::Array(vec![
            lopdf::Object::string_literal("0123456789abcdef"),
            lopdf::Object::string_literal("0123456789abcdef"),
        ]),
    );
    let encryption = EncryptionState::try_from(EncryptionVersion::V2 {
        document: &document,
        owner_password: "owner",
        user_password: "",
        key_length: 128,
        permissions: Permissions::all(),
    })
    .unwrap();
    document.encrypt(&encryption).unwrap();
    let locked = folder.join("locked.pdf");
    document.save(&locked).unwrap();
    run.ok(&["open", plain.to_str().unwrap()]);
    run.settled();
    run.ok(&["page", "insert", locked.to_str().unwrap(), "--at", "1"]);
    run.settled();
    assert_eq!(run.ok(&["edits"])["pages"], 3);
    let on_plain = format!("#{}", tabs_of(&run.report())[0]["id"]);
    run.ok(&["open", other.to_str().unwrap()]);
    run.settled();
    run.ok(&["session", "sleep", "--tab", &on_plain]);
    let colours = colours_in_sleep_files(&state);
    assert!(colours.iter().all(|(_, colours)| colours.is_empty()), "no pictures at all: {colours:?}");
    // And on a page of its own too, as a tab on the file itself.
    run.ok(&["open", locked.to_str().unwrap()]);
    run.settled();
    run.ok(&["open", other.to_str().unwrap()]);
    run.settled();
    let on_locked = format!(
        "#{}",
        tabs_of(&run.report())
            .iter()
            .find(|tab| tab["path"].as_str().unwrap().ends_with("locked.pdf"))
            .unwrap()["id"]
    );
    run.ok(&["session", "sleep", "--tab", &on_locked]);
    run.hand_off();
    assert!(
        colours_in_sleep_files(&state).iter().all(|(_, colours)| !colours.contains(&"red")),
        "{:?}",
        colours_in_sleep_files(&state)
    );
}

/// Failure (review): a forged sleep file says a 3-page PDF has 100 pages, and that goes on being believed through drawing and another sleep, and a rotation lands on a page that isn't there. The worker's word goes over it as soon as it has the file open: the sleep file goes, and the file's read.
#[test]
fn a_forged_length_lasts_only_till_the_worker_has_the_file() {
    let state = State::at(Level::Full);
    let Some(run) = Run::on(&state) else { return };
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let (short, other) = (text_document(&folder, "short.pdf", 3), text_document(&folder, "other.pdf", 2));
    run.ok(&["open", short.to_str().unwrap()]);
    run.settled();
    run.ok(&["open", other.to_str().unwrap()]);
    run.settled();
    let on_short = format!("#{}", tabs_of(&run.report())[0]["id"]);
    run.ok(&["session", "sleep", "--tab", &on_short]);
    let file = sleep_files(&state).pop().unwrap();
    let mut sleep = recto_session::sleep::decode_sleep(&std::fs::read(&file).unwrap(), recto_session::record::CURRENT_VERSION).unwrap();
    sleep.file_pages = 100;
    sleep.pages = 100;
    std::fs::write(&file, recto_session::sleep::encode_sleep(&sleep)).unwrap();
    run.ok(&["tap", &on_short]);
    wait_until("it's found out", || run.ok(&["where"])["pages"] == 3);
    run.settled();
    wait_until("the forged file goes", || sleep_files(&state).is_empty());
    let (refused, code) = state.json(&["page", "rotate", "50", "--degrees", "90"]);
    assert_ne!(code, 0, "no page 50: {refused}");
    run.ok(&["tap", "#2"]);
    run.settled();
    run.ok(&["session", "sleep", "--tab", &on_short]);
    let again = recto_session::sleep::decode_sleep(
        &std::fs::read(sleep_files(&state).pop().unwrap()).unwrap(),
        recto_session::record::CURRENT_VERSION,
    )
    .unwrap();
    assert_eq!(again.file_pages, 3, "what's written next is true");
}

/// Failure (review L4): a file kept for undoing past a save is gone while its list is asleep. The steps needing it roll off on waking, with the journal still saying them, so `session save` and a hand-off are refused until a restart. And `session show` says there's nothing to undo on a list that's asleep.
#[test]
fn a_kept_file_gone_while_asleep_doesnt_stop_a_save() {
    let state = State::at(Level::Full);
    let Some(run) = Run::on(&state) else { return };
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let (file, other) = (text_document(&folder, "doc.pdf", 3), text_document(&folder, "other.pdf", 2));
    run.ok(&["open", file.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "owner-edit", "delete:2"]);
    run.settled();
    let written = folder.join("written.pdf");
    written_without(&file, &[2], &written);
    run.ok(&["qa", "save-from", written.to_str().unwrap()]);
    run.settled();
    let on_doc = format!("#{}", tabs_of(&run.report())[0]["id"]);
    run.ok(&["open", other.to_str().unwrap()]);
    run.settled();
    let slept = run.ok(&["session", "sleep", "--tab", &on_doc]);
    assert_eq!(slept["list_asleep"], true, "{slept}");
    let shown = run.ok(&["session", "show"]);
    assert_eq!(shown["tabs"][0]["can_undo"], true, "asleep, and there's still a step: {shown}");
    let journals = state.root.join("session").join("journals");
    let kept = std::fs::read_dir(&journals)
        .unwrap()
        .flatten()
        .find(|entry| entry.file_name().to_string_lossy().starts_with("kept-"))
        .unwrap()
        .path();
    std::fs::remove_file(kept).unwrap();
    run.ok(&["tap", &on_doc]);
    run.settled();
    assert_eq!(tabs_of(&run.report())[0]["notice"], "history_rolled_off");
    assert_eq!(run.ok(&["edits"])["can_undo"], false);
    let saved = run.ok(&["session", "save"]);
    assert_eq!(saved["saved"], true, "{saved}");
    let handed = run.hand_off();
    assert!(handed.get("error").is_none(), "{handed}");
}

/// Failure: a sleep file's believed when it's been tampered with, or about a file that's since changed, so a tab wakes to a stranger's contents and pictures; or a bad one stops it opening. It's thrown away, and the tab reads its PDF, as if there'd been none.
#[test]
fn a_sleep_file_thats_wrong_only_costs_the_quick_way() {
    let state = State::at(Level::Full);
    let Some(run) = Run::on(&state) else { return };
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let (one, two) = (text_document(&folder, "one.pdf", 5), text_document(&folder, "two.pdf", 3));
    run.ok(&["open", one.to_str().unwrap()]);
    run.settled();
    run.ok(&["open", two.to_str().unwrap()]);
    run.settled();
    let on = |name: &str, run: &Run<'_>| {
        tabs_of(&run.report()).iter().find(|tab| tab["path"].as_str().unwrap().ends_with(name)).unwrap()["id"]
            .as_u64()
            .unwrap()
    };
    let first = on("one.pdf", &run);
    run.ok(&["session", "sleep", "--tab", &format!("#{first}")]);
    wait_until("the file's there", || sleep_files(&state).len() == 1);
    // Damaged.
    let file = sleep_files(&state).pop().unwrap();
    let mut bytes = std::fs::read(&file).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x55;
    std::fs::write(&file, &bytes).unwrap();
    run.ok(&["tap", &format!("#{first}")]);
    run.settled();
    assert_eq!(run.report()["reader"]["last_wake"]["from_sleep"], false);
    assert_eq!(run.ok(&["where"])["pages"], 5, "it's read from its PDF");
    assert!(sleep_files(&state).is_empty(), "and it's gone");
    // Changed while it slept: the old one's found, shown for a moment, then seen to be wrong on the way past.
    run.ok(&["tap", &format!("#{}", on("two.pdf", &run))]);
    run.settled();
    run.ok(&["session", "sleep", "--tab", &format!("#{first}")]);
    wait_until("the file's there again", || sleep_files(&state).len() == 1);
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(
        &one,
        pages(&(0..9).map(|at| Page::new(text_lines(&[&format!("New {at}")]))).collect::<Vec<_>>()),
    )
    .unwrap();
    run.ok(&["tap", &format!("#{first}")]);
    wait_until("it finds the new one", || run.ok(&["where"])["pages"] == 9);
    run.settled();
    assert_eq!(
        tabs_of(&run.report()).iter().find(|tab| tab["id"] == first).unwrap()["notice"],
        "changed",
        "it says so, with no edits too (decision 23): {}",
        run.report()["session"]["tabs"]
    );
    let mut gone = false;
    for _ in 0..200 {
        gone = sleep_files(&state).is_empty();
        if gone {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        gone,
        "its old picture's still there: {:?} {}",
        sleep_files(&state),
        run.report()["record"]["sleep"]
    );
}

/// Failure: a quiet run acts on the machine's own memory pressure, so a test's tabs go to sleep under it when other work loads the Mac (full check, 2026-11-12: the test below saw 2 documents held of 3, during the kernel's only warning that hour); or it isn't counted, so a run can't say the machine was under pressure. Through the method the host's dispatch source calls.
#[test]
fn a_quiet_run_counts_the_systems_memory_pressure_and_puts_nothing_to_sleep() {
    let state = State::at(Level::Full);
    let Some(run) = Run::on(&state) else { return };
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    for at in 0..2 {
        run.ok(&["open", text_document(&folder, &format!("d{at}.pdf"), 2).to_str().unwrap()]);
        run.settled();
    }
    run.ok(&["qa", "memory-pressure", "system-warning"]);
    run.settled();
    let sleep = &run.report()["record"]["sleep"];
    assert!(sleep["system_pressure"]["warning"].as_u64().unwrap() >= 1, "counted: {sleep}");
    assert_eq!(sleep["pressure_events"], 0, "and not acted on: {sleep}");
    assert_eq!(sleep["gone_dormant"]["pressure"], 0, "{sleep}");
}

/// Failure: the system says memory's short and tabs off screen stay, or the budget across documents isn't kept, or what's being read goes blank. Everything off screen goes dormant at once, oldest first, and what's on screen stays. And a window of many tabs can't go over its budget.
#[test]
fn pressure_and_the_budget_do_what_being_left_alone_does() {
    let state = State::at(Level::Full);
    let Some(run) = Run::on(&state) else { return };
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let files: Vec<PathBuf> = (0..3).map(|at| text_document(&folder, &format!("d{at}.pdf"), 4)).collect();
    for file in &files {
        run.ok(&["open", file.to_str().unwrap()]);
        run.settled();
    }
    assert_eq!(run.report()["reader"]["documents_held"], 3);
    run.ok(&["qa", "memory-pressure", "warning"]);
    wait_until("they go", || run.report()["reader"]["documents_held"] == 1);
    let after = run.report();
    assert_eq!(after["record"]["sleep"]["pressure_events"], 1);
    assert_eq!(after["record"]["sleep"]["gone_dormant"]["pressure"], 2);
    run.settled();
    assert!(
        run.report()["reader"]["tiles"]["attached"].as_u64().unwrap() > 0,
        "and what's on screen is still there"
    );
    // Over the budget, however it's set: a document off screen holds its worker, so any at all is over nothing.
    run.ok(&["qa", "live-budget", "0"]);
    let tabs = tabs_of(&run.report());
    run.ok(&["tap", &format!("#{}", tabs[0]["id"])]);
    run.settled();
    run.ok(&["tap", &format!("#{}", tabs[1]["id"])]);
    run.settled();
    wait_until("the one with a worker goes", || {
        run.report()["record"]["sleep"]["gone_dormant"]["budget"].as_u64().unwrap() >= 1
    });
}

/// Failure (decision 24): a launch or a hand-off waits for sharp pages before there's a window, which on a heavy page is most of a second; or its first frame's anything but the place with something on every page. It goes up from the sleep file with the picture on, and the file's not rewritten when nothing's changed.
#[test]
fn a_launch_goes_up_from_the_sleep_file() {
    let state = State::at(Level::Full);
    let Some(run) = Run::on(&state) else { return };
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let file = text_document(&folder, "doc.pdf", 9);
    run.ok(&["open", file.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "4"]);
    run.settled();
    let before = run.report();
    run.hand_off();
    assert_eq!(sleep_files(&state).len(), 1, "the hand-off wrote it");
    let written = std::fs::read(&sleep_files(&state)[0]).unwrap();
    let run = Run::on(&state).unwrap();
    run.settled();
    let after = run.report();
    let shown = &after["record"]["first_shown"];
    assert_eq!(shown["page"], before["reader"]["page"], "{shown}");
    assert!(
        (shown["scroll_y"].as_f64().unwrap() - before["reader"]["scroll_y"].as_f64().unwrap()).abs() < 1.0,
        "{shown}"
    );
    assert!(shown["tile_layers"].as_u64().unwrap() > 0, "something on it: {shown}");
    assert_eq!(
        after["reader"]["last_wake"]["from_sleep"],
        true,
        "{} {} {} {}",
        after["reader"]["last_wake"],
        after["record"]["sleep"],
        after["reader"]["tiles"],
        written.len()
    );
    assert_eq!(after["reader"]["last_wake"]["blank_frames"], 0);
    assert_eq!(after["record"]["sleep"]["woken_from_file"], 1);
    // Nothing's changed, so a quit writes nothing to it.
    let modified = std::fs::metadata(&sleep_files(&state)[0]).unwrap().modified().unwrap();
    run.quit();
    assert_eq!(std::fs::metadata(&sleep_files(&state)[0]).unwrap().modified().unwrap(), modified);
    assert_eq!(std::fs::read(&sleep_files(&state)[0]).unwrap(), written);
}

/// What a Save would write, for this test: the document's pages, as lopdf has them.
#[allow(clippy::disallowed_methods)] // The stand-in for a save that deletes pages, until the rewrite saves those.
fn written_without(source: &Path, delete: &[u32], to: &Path) {
    let mut document = lopdf::Document::load(source).unwrap();
    document.delete_pages(delete);
    document.prune_objects();
    document.save(to).unwrap();
}

/// Failure (decision 25), in the real app: after a Save, undo doesn't go past it, or it draws the wrong page, as the deleted page isn't in the file any more. Or the document reads wrongly on either side of the saved point, or any of that doesn't survive a kill. Or Clear Edit History leaves its files on disk, or an agent gets to clear the owner's history.
#[test]
fn undo_goes_past_a_save_in_the_real_app() {
    let state = State::at(Level::Full);
    let Some(run) = Run::on(&state) else { return };
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("doc.pdf");
    std::fs::write(
        &file,
        pages(&["Alpha page", "Bravo page", "Charlie page"].map(|text| Page::new(text_lines(&[text])))),
    )
    .unwrap();
    let text_of = |run: &Run<'_>, page: &str| run.ok(&["text", "--page", page]).to_string();
    let edited = |run: &Run<'_>| run.ok(&["edits"])["edited"].as_bool().unwrap();
    run.ok(&["open", file.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "owner-edit", "delete:2"]);
    run.settled();
    assert!(edited(&run));
    let written = folder.join("written.pdf");
    written_without(&file, &[2], &written);
    run.ok(&["qa", "save-from", written.to_str().unwrap()]);
    run.settled();
    assert!(!edited(&run), "it reads as saved");
    assert_eq!(run.ok(&["edits"])["file_pages"], 2);
    let second = text_of(&run, "2");
    assert!(second.contains("Charlie"), "the file's second page now: {second:?} {}", run.ok(&["edits"]));
    let journals = state.root.join("session").join("journals");
    let kept_files = || {
        std::fs::read_dir(&journals)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("kept-"))
            .count()
    };
    assert_eq!(kept_files(), 1, "the page it took out is kept");
    let journal_bytes = || {
        std::fs::read_dir(&journals)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("journal-"))
            .map(|entry| entry.metadata().unwrap().len())
            .sum::<u64>()
    };
    wait_until("the new journal goes at once", || journal_bytes() > 0);
    let before_undo = journal_bytes();
    // Past the saved point.
    run.ok(&["qa", "owner-edit", "undo"]);
    run.settled();
    assert!(edited(&run), "unsaved again");
    assert_eq!(run.ok(&["edits"])["pages"], 3);
    assert!(text_of(&run, "2").contains("Bravo"), "the page that's only in what was kept");
    assert!(text_of(&run, "3").contains("Charlie"), "and the rest from the new file");
    // Across a kill, past the saved point, and back to it.
    wait_until("the undo reaches the journal", || journal_bytes() > before_undo);
    run.kill();
    let run = Run::on(&state).unwrap();
    run.settled();
    assert!(edited(&run), "still past it");
    assert!(text_of(&run, "2").contains("Bravo"));
    run.ok(&["qa", "owner-edit", "redo"]);
    run.settled();
    assert!(!edited(&run), "back at the saved point, it reads as saved");
    assert_eq!(run.ok(&["edits"])["pages"], 2);
    assert!(text_of(&run, "2").contains("Charlie"));
    // Clearing: at Control the owner's history is refused, and nothing goes.
    state.set_level(Level::Control);
    let (refused, _) = state.json(&["clear-history"]);
    assert_eq!(refused["error"]["code"], "would_discard_user_edits", "{refused}");
    assert_eq!(kept_files(), 1);
    state.set_level(Level::Full);
    let cleared = run.ok(&["clear-history"]);
    assert_eq!(cleared["can_undo"], false, "{cleared}");
    run.settled();
    wait_until("its files go at once", || {
        kept_files() == 0
            && std::fs::read_dir(&journals)
                .unwrap()
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().starts_with("journal-"))
    });
    let (nothing, _) = state.json(&["undo"]);
    assert_eq!(nothing["error"]["code"], "nothing_to_undo", "{nothing}");
    assert!(run.ok(&["clear-history"])["did"].as_str().unwrap().contains("no edit history"));
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let started = std::time::Instant::now();
    while !done() {
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "{what}");
        std::thread::sleep(std::time::Duration::from_millis(20)); // ratchet: poll
    }
}

fn wait_for_journal(run: &Run<'_>) -> Value {
    let began = Instant::now();
    while run.ok(&["session", "show"])["record"]["appends"] == 0 {
        assert!(began.elapsed() < Duration::from_secs(20), "the journal never went out");
        std::thread::sleep(Duration::from_millis(50));
    }
    run.report()["session"]["edits_full"].clone()
}

/// A tab's list, as `qa state` has it by the tab's number.
fn tabs_of(report: &Value) -> Vec<Value> {
    report["session"]["tabs"].as_array().unwrap().clone()
}

/// Failure (review B1): a first edit on a tab the record already has, then a kill within a place's minute. The journal goes out in seconds and the record has no name for it yet, so it came back as a second tab on the same PDF, the first one clean and active, the edits on the other at page 1. It's one tab, where it was, with its edits.
#[test]
fn a_first_edit_then_a_kill_keeps_one_tab() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let file = long_document("first-edit", 12);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "7"]);
    run.settled();
    // The record goes out, and then nothing else does for a minute.
    run.ok(&["qa", "frontmost", "off"]);
    run.quit();
    let run = Run::on(&state).unwrap();
    run.settled();
    run.ok(&["qa", "owner-edit", "rotate:2"]);
    let lists = wait_for_journal(&run);
    run.kill();
    let run = Run::on(&state).unwrap();
    run.settled();
    let after = run.report();
    assert_eq!(tabs_of(&after).len(), 1, "{}", after["session"]["tabs"]);
    assert_eq!(after["session"]["edits_full"], lists);
    assert_eq!(after["reader"]["page"], 7, "and it's where it was");
}

/// Failure (review B2): an edit on a fresh folder, with no record yet, then a kill inside the first minute. The app came back empty, and opening the PDF and editing again wiped the journal, so the edit was gone for good. It's the very first edit of a new install, and it comes back.
#[test]
fn a_kill_before_any_record_keeps_the_edits() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let file = long_document("no-record", 9);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "owner-edit", "delete:3"]);
    let lists = wait_for_journal(&run);
    assert!(
        !state.root.join("session/record.a.json").exists() && !state.root.join("session/record.b.json").exists(),
        "this is about having no record"
    );
    run.kill();
    let run = Run::on(&state).unwrap();
    run.settled();
    let after = run.report();
    assert_eq!(tabs_of(&after).len(), 1, "{}", after["session"]["tabs"]);
    let back = after["session"]["edits_full"].as_object().unwrap().values().next().cloned();
    assert_eq!(back.as_ref(), lists.as_object().unwrap().values().next(), "the edit and whose it was");
}

/// Failure (review B3): a tab closes within the journal's wait, with the undo not yet written. Closing dropped what hadn't gone out while the file stayed, so an undone edit came back live, as a tab, on the next launch. Nothing's lost by closing, and what's undone stays undone.
#[test]
fn closing_loses_no_operation_on_its_way_out() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let file = long_document("closing", 6);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["page", "rotate", "1", "--by", "90"]);
    wait_for_journal(&run);
    run.ok(&["undo"]);
    run.ok(&["close"]);
    run.quit();
    let run = Run::on(&state).unwrap();
    run.settled();
    let after = run.report();
    assert_eq!(tabs_of(&after).len(), 0, "{}", after["session"]["tabs"]);
    // And the PDF opened again has its way forward.
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let list = run.report()["session"]["edits_full"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .cloned()
        .expect("its history");
    assert_eq!(
        (list["undo"].as_array().unwrap().len(), list["redo"].as_array().unwrap().len()),
        (0, 1),
        "{list}"
    );
}

/// Failure (review B4): owner history with nothing unsaved, as four quarter turns are, is what a close allows and keeps. It brought the tab back on every launch, under a new number each time, however many times it was closed. A closed tab stays closed, and its history waits for the PDF to be opened.
#[test]
fn a_closed_tab_stays_closed_and_keeps_its_history() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let file = long_document("closed", 6);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    for _ in 0..4 {
        run.ok(&["qa", "owner-edit", "rotate:1"]);
    }
    run.ok(&["close"]);
    run.quit();
    for _ in 0..2 {
        let run = Run::on(&state).unwrap();
        run.settled();
        assert_eq!(tabs_of(&run.report()).len(), 0);
        run.quit();
    }
    let run = Run::on(&state).unwrap();
    run.settled();
    // By the app's own route: a drop, the menu, a launch argument.
    run.ok(&["qa", "open-file", file.0.to_str().unwrap()]);
    run.settled();
    let list = run.report()["session"]["edits_full"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .cloned()
        .expect("its history");
    assert_eq!(list["undo"].as_array().unwrap().len(), 4, "{list}");
}

/// Failure (review B5): `recto open` didn't pick up a closed PDF's history as the app's own open did, so the next edit went on the end of the old journal. What was on disk stopped matching what was in memory, and every save and hand-off was refused. The same route either way, by rule 4, and the same history.
#[test]
fn opening_by_either_route_picks_up_the_history() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let file = long_document("routes", 8);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    for _ in 0..4 {
        run.ok(&["qa", "owner-edit", "rotate:1"]);
    }
    run.ok(&["close"]);
    run.quit();
    let run = Run::on(&state).unwrap();
    run.settled();
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "owner-edit", "delete:5"]);
    let list = run.report()["session"]["edits_full"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .cloned()
        .expect("a list");
    assert_eq!(list["undo"].as_array().unwrap().len(), 5, "its 4 and the new one: {list}");
    run.ok(&["session", "save"]);
    let lists = run.report()["session"]["edits_full"].clone();
    run.kill();
    let run = Run::on(&state).unwrap();
    run.settled();
    let back = run.report()["session"]["edits_full"].clone();
    assert_eq!(back.as_object().unwrap().values().next(), lists.as_object().unwrap().values().next());
    assert_eq!(tabs_of(&run.report()).len(), 1);
}

/// Failure (decision 26, and review B7): a second tab through a link has the same file, so it shared a journal: an edit on it landed in the first's file, and after a restart the first had it and the second was unreadable. "A PDF can only be opened once": a link goes to the tab, by any route, and `recto open` says so. A copy is another file, and a PDF added as pages isn't a tab.
#[test]
fn a_pdf_can_only_be_opened_once_by_any_route() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let file = long_document("once", 8);
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let (link, hard, copy) = (folder.join("link.pdf"), folder.join("hard.pdf"), folder.join("copy.pdf"));
    std::os::unix::fs::symlink(&file.0, &link).unwrap();
    std::fs::copy(&file.0, &copy).unwrap();
    let first = run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    assert!(first.get("already_open").is_none(), "{first}");
    let again = run.ok(&["open", link.to_str().unwrap()]);
    assert_eq!(again["tab"], first["tab"]);
    assert_eq!(again["already_open"], true, "{again}");
    // The app's own route: a drop, the menu, a launch argument.
    run.ok(&["qa", "open-file", link.to_str().unwrap()]);
    run.settled();
    assert_eq!(tabs_of(&run.report()).len(), 1);
    let other = run.ok(&["open", copy.to_str().unwrap()]);
    assert_ne!(other["tab"], first["tab"], "a copy is another file");
    run.settled();
    // A hard link, on the same volume as what it's a link to.
    if std::fs::hard_link(&copy, &hard).is_ok() {
        assert_eq!(run.ok(&["open", hard.to_str().unwrap()])["tab"], other["tab"]);
    }
    assert_eq!(tabs_of(&run.report()).len(), 2);
    // As pages it's allowed, with no tab for it.
    run.ok(&["page", "insert", link.to_str().unwrap(), "--at", "1"]);
    assert_eq!(tabs_of(&run.report()).len(), 2);
    // Moved, and opened by its new name, the tab goes with it.
    let moved = folder.join("moved.pdf");
    std::fs::rename(&copy, &moved).unwrap();
    let _ = std::fs::remove_file(&hard);
    run.ok(&["tap", &format!("#{}", first["tab"])]);
    run.settled();
    let found = run.ok(&["open", moved.to_str().unwrap()]);
    assert_eq!(found["tab"], other["tab"]);
    run.settled();
    let report = run.report();
    assert_eq!(report["session"]["tabs"][1]["path"], moved.to_str().unwrap(), "{}", report["session"]["tabs"]);
}

/// Failure (final review N1): a `cp -p` copy has the same size and time as what it's a copy of, and both are open. A first edit on the copy, and a kill before the record names its journal: the edits came back on the original, as the first tab with the same size and time. It's a different file, by its path and inode, and they come back where they were made.
#[test]
fn a_journal_doesnt_go_to_a_copy_of_its_pdf() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let (original, copy) = (folder.join("a.pdf"), folder.join("acopy.pdf"));
    std::fs::write(
        &original,
        pages(&(0..6).map(|at| Page::new(text_lines(&[&format!("Page {at}")]))).collect::<Vec<_>>()),
    )
    .unwrap();
    assert!(Command::new("/bin/cp").arg("-p").arg(&original).arg(&copy).status().unwrap().success());
    run.ok(&["open", original.to_str().unwrap()]);
    run.ok(&["open", copy.to_str().unwrap()]);
    run.settled();
    // Both in the record, then nothing else goes for a minute.
    run.quit();
    let run = Run::on(&state).unwrap();
    run.settled();
    let tabs = tabs_of(&run.report());
    assert_eq!(tabs.len(), 2);
    let on_copy = tabs.iter().find(|tab| tab["path"].as_str().unwrap().ends_with("acopy.pdf")).unwrap()["id"]
        .as_u64()
        .unwrap();
    run.ok(&["tap", &format!("#{on_copy}")]);
    run.settled();
    run.ok(&["qa", "owner-edit", "delete:3"]);
    wait_for_journal(&run);
    run.kill();
    let run = Run::on(&state).unwrap();
    run.settled();
    let after = run.report();
    assert_eq!(tabs_of(&after).len(), 2, "{}", after["session"]["tabs"]);
    let lists = after["session"]["edits_full"].as_object().unwrap();
    assert_eq!(
        lists.keys().collect::<Vec<_>>(),
        [&on_copy.to_string()],
        "on the copy, where they were made: {}",
        after["session"]["tabs"]
    );
}

/// Failure (final review N7): a closed PDF's history is taken up when its file couldn't even be read, so it's marked as open with nothing holding it. Opening again and editing wrote over the end of the old journal, and every save and hand-off failed with what's written not reading back. It's only taken after a good read.
#[test]
fn history_is_only_taken_up_after_a_good_read() {
    use std::os::unix::fs::PermissionsExt;
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("a.pdf");
    std::fs::write(
        &file,
        pages(&(0..6).map(|at| Page::new(text_lines(&[&format!("Page {at}")]))).collect::<Vec<_>>()),
    )
    .unwrap();
    run.ok(&["open", file.to_str().unwrap()]);
    run.settled();
    for _ in 0..4 {
        run.ok(&["qa", "owner-edit", "rotate:1"]);
    }
    run.ok(&["close"]);
    run.ok(&["session", "save"]);
    let began = Instant::now();
    while !std::fs::read_dir(state.root.join("session/journals"))
        .unwrap()
        .flatten()
        .any(|entry| entry.file_name().to_string_lossy().starts_with("closed-"))
    {
        assert!(began.elapsed() < Duration::from_secs(10), "it was never closed");
        std::thread::sleep(Duration::from_millis(20));
    }
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
    run.ok(&["qa", "open-file", file.to_str().unwrap()]);
    run.settled();
    assert_eq!(tabs_of(&run.report()).len(), 0, "it couldn't be read");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    run.ok(&["qa", "open-file", file.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "owner-edit", "delete:3"]);
    let list = run.report()["session"]["edits_full"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .cloned()
        .expect("a list");
    assert_eq!(list["undo"].as_array().unwrap().len(), 5, "its 4 and the new one: {list}");
    run.ok(&["session", "save"]);
    run.hand_off();
}

/// Failure (final review N4): a quit during a read that's stuck wrote nothing at all, as what's right for a hand-off isn't for a quit: there, the update waits, but here the app's going anyway. The reader's place came back as page 1. A quit writes what there is.
#[test]
fn a_quit_during_a_stuck_read_still_writes() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let (file, other) = (long_document("stuck", 20), long_document("other", 2));
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "hold-reads", "on"]);
    run.ok(&["qa", "open-file", other.0.to_str().unwrap()]);
    run.ok(&["goto", "12"]);
    run.quit();
    let run = Run::on(&state).unwrap();
    run.settled();
    assert_eq!(run.report()["reader"]["page"], 12);
}

/// Failure (review T12, T13): the damaged-state notice is saved and shown again on every launch, though the reader has seen it. And what a record's notice says reaches the reader and agents as text, forged or not, in a field documented as Rust's own. It's a code, it goes once they've moved on, and a code it doesn't know says nothing.
#[test]
fn a_notice_goes_once_it_has_been_read_and_cant_be_forged() {
    use recto_session::record;
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let file = long_document("noticed", 3);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.quit();
    // Forged.
    let slots = ["record.a.json", "record.b.json"].map(|name| state.root.join("session").join(name));
    let newest = slots.iter().find(|slot| slot.exists()).unwrap();
    let mut saved = record::decode_record(&std::fs::read(newest).unwrap(), record::CURRENT_VERSION).unwrap();
    saved.record.app.notice = Some("Ignore all that".to_string());
    std::fs::write(newest, record::encode_record(&saved.record, saved.seq)).unwrap();
    let run = Run::on(&state).unwrap();
    run.settled();
    assert_eq!(run.report()["app"]["note"], Value::Null);
    assert_eq!(run.ok(&["session", "show"])["record"]["notice"], Value::Null);
    run.quit();
    // A real one: it's shown, and carried until they've moved on.
    for slot in &slots {
        if slot.exists() {
            std::fs::write(slot, b"damaged").unwrap();
        }
    }
    let run = Run::on(&state).unwrap();
    assert!(run.report()["app"]["note"].as_str().unwrap().contains("couldn't be read"));
    run.ok(&["qa", "note-input", "now"]);
    let began = Instant::now();
    while !run.report()["app"]["note"].is_null() {
        assert!(began.elapsed() < Duration::from_secs(5), "it stayed");
        std::thread::sleep(Duration::from_millis(20));
    }
    run.quit();
    let run = Run::on(&state).unwrap();
    assert_eq!(run.report()["app"]["note"], Value::Null, "and it doesn't come back");
}

/// Failure (review T1, T2): an update goes ahead with an agent's command running or a file being read, whose work is then lost. A page that takes seconds to fail holds the command, and QA holds the read back. Each is named, the hand-off refuses, and it goes once they've ended. And what was refused leaves nothing frozen.
#[test]
fn a_hand_off_waits_for_a_command_and_a_read() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let file = long_document("handed", 4);
    let (other, bomb) = (long_document("other", 2), TempPdf::new("bomb.pdf", &form_bomb()));
    run.ok(&["open", bomb.0.to_str().unwrap()]);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    // An agent's command, under way on a job thread.
    let out = state.root.join("bomb.png");
    let mut waiting = state
        .command()
        .args(["render", "1", "--tab", "#1", "--out", out.to_str().unwrap(), "--json"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let began = Instant::now();
    let check = loop {
        let check = run.ok(&["session", "check"]);
        if check["safe"] == false {
            break check;
        }
        assert!(began.elapsed() < Duration::from_secs(10), "{check}");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(check["busy"][0]["code"], "agent_command", "{check}");
    let (reply, code) = state.json(&["qa", "handoff", "now"]);
    assert_ne!(code, 0, "{reply}");
    assert!(reply["error"]["message"].as_str().unwrap().contains("agent"), "{reply}");
    let _ = waiting.wait();
    assert_eq!(run.ok(&["session", "check"])["safe"], true);
    // A file being opened by the app's own route, held back here.
    run.ok(&["qa", "hold-reads", "on"]);
    run.ok(&["qa", "open-file", other.0.to_str().unwrap()]);
    let began = Instant::now();
    let check = loop {
        let check = run.ok(&["session", "check"]);
        if check["safe"] == false {
            break check;
        }
        assert!(began.elapsed() < Duration::from_secs(10), "{check}");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(check["busy"][0]["code"], "file_opening", "{check}");
    let (reply, code) = state.json(&["qa", "handoff", "now"]);
    assert_ne!(code, 0, "{reply}");
    assert!(process_counters(run.pid).is_some(), "it's still here");
    run.ok(&["qa", "hold-reads", "off"]);
    run.settled();
    assert_eq!(run.ok(&["session", "check"])["safe"], true);
    // A quit for an update that hasn't happened does nothing.
    assert_eq!(run.ok(&["qa", "hand-off-quit", "now"])["quit"], false);
    assert!(process_counters(run.pid).is_some(), "still here");
    // What was refused left nothing frozen: it's still working.
    run.ok(&["goto", "1"]);
    run.settled();
    assert_eq!(tabs_of(&run.report()).len(), 3);
    run.hand_off();
}

/// Failure: the sidebar hidden by its button comes back shown, or its button doesn't come back to bring it back, or a tab's layout comes back as another tab's or as a page to a row, across a quit and a hand-off alike. The rich state's comparison has the layouts; this has the sidebar hidden, which leaves the contents unlaid until it's shown again.
#[test]
fn the_hidden_sidebar_and_each_tabs_layout_come_back() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let (one, two) = (long_document("one", 9), long_document("two", 9));
    run.ok(&["open", one.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["view", "--layout", "two-up-cover"]);
    run.ok(&["open", two.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["view", "--layout", "two-up", "--sidebar", "off"]);
    run.ok(&["goto", "6"]);
    run.settled();
    let before = run.report();
    run.quit();
    let run = Run::on(&state).unwrap();
    run.settled();
    let after = run.report();
    assert_eq!(
        (
            &after["app"]["sidebar_hidden"],
            &after["app"]["sidebar_visible"],
            &after["app"]["sidebar_button"]["visible"]
        ),
        (&json!(true), &json!(false), &json!(true)),
        "{}",
        after["app"]
    );
    assert_eq!(after["app"]["layout"], "two-up");
    assert_eq!(after["reader"]["page"], 6);
    assert_eq!(after["app"]["pages"]["scroll_y"], before["app"]["pages"]["scroll_y"], "to the pixel");
    let layouts: Vec<&Value> = after["session"]["tabs"].as_array().unwrap().iter().map(|tab| &tab["layout"]).collect();
    assert_eq!(layouts, [&json!("two-up-cover"), &json!("two-up")]);
    run.hand_off();
    let run = Run::on(&state).unwrap();
    run.settled();
    let handed = run.report();
    assert_eq!(
        (&handed["app"]["sidebar_hidden"], &handed["app"]["layout"]),
        (&json!(true), &json!("two-up")),
        "{}",
        handed["app"]
    );
    run.ok(&["qa", "button", "sidebar"]);
    let shown = run.report();
    assert_eq!(shown["app"]["sidebar_visible"], true, "{}", shown["app"]);
}

/// Failure: what's selected is lost across a restart: SPEC decision 23 has it as part of what comes back. On the page it was on, the same characters, and highlighted again.
#[test]
fn a_selection_comes_back() {
    selection_comes_back(&[]);
}

/// Failure (late-restore-2): with the state read late, the selection came back on the window that went up without it and went with that window when it was made again from the state, and so did Find's hit. Selected again on the new one.
#[test]
fn a_selection_comes_back_when_the_state_is_read_late() {
    selection_comes_back(&[("RECTO_QA_RESTORE_DELAY_MS", std::ffi::OsStr::new("1500"))]);
}

fn selection_comes_back(second_launch: &[(&str, &std::ffi::OsStr)]) {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let file = long_document("selected", 8);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "2"]);
    run.settled();
    run.ok(&["qa", "menu", "edit.select_all"]);
    run.settled();
    let before = run.report();
    assert_eq!(before["reader"]["selection"]["selected"], true, "{}", before["reader"]["selection"]);
    run.quit();
    let run = Run::on_with_env(&state, second_launch).unwrap();
    run.settled();
    let after = run.report();
    assert_eq!(after["reader"]["selection"]["selected"], true, "{}", after["reader"]["selection"]);
    assert_eq!(after["reader"]["selection"]["start"], before["reader"]["selection"]["start"]);
    assert_eq!(after["reader"]["selection"]["end"], before["reader"]["selection"]["end"]);
    assert_eq!(after["session"]["tabs"][0]["selection"], before["session"]["tabs"][0]["selection"]);
}

/// Failure: a `Moved` notice is on a tab with edits, and a restart drops it, or anything else about that tab differs. What's carried is compared whole, as the rich state is.
#[test]
fn a_moved_file_and_its_notice_come_back() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let folder = state.root.join("docs");
    std::fs::create_dir_all(folder.join("there")).unwrap();
    let path = folder.join("a.pdf");
    std::fs::write(
        &path,
        pages(&(0..6).map(|at| Page::new(text_lines(&[&format!("Page {at}")]))).collect::<Vec<_>>()),
    )
    .unwrap();
    run.ok(&["open", path.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "owner-edit", "rotate:2"]);
    run.ok(&["qa", "scroller-style", "overlay"]);
    run.quit();
    std::fs::rename(&path, folder.join("there/b.pdf")).unwrap();
    let run = Run::on(&state).unwrap();
    run.ok(&["qa", "scroller-style", "overlay"]);
    run.settled();
    let before = run.report();
    assert_eq!(
        before["session"]["tabs"][0]["notice"]["moved"]["from"],
        path.to_str().unwrap(),
        "{}",
        before["session"]["tabs"]
    );
    run.quit();
    let run = Run::on(&state).unwrap();
    run.ok(&["qa", "scroller-style", "overlay"]);
    run.settled();
    assert_same(&before, &run.report(), &[]);
}

/// Failure: killed outright mid-work, the app comes back with nothing, or a broken record. What had gone to disk by then is there: edits past their wait, whose each is, and the record's whole however it was caught.
#[test]
fn killed_outright_it_comes_back_with_what_was_written() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let (a, b) = (long_document("alpha", 30), long_document("beta", 10));
    run.ok(&["open", a.0.to_str().unwrap()]);
    run.ok(&["open", b.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["tap", "#1"]);
    run.settled();
    run.ok(&["qa", "owner-edit", "rotate:1"]);
    run.ok(&["page", "delete", "2"]);
    // Nothing forces the record out here: a kill has to work from the first edit on, within a place's minute.
    let lists = wait_for_journal(&run);
    run.kill();
    let run = Run::on(&state).unwrap();
    run.settled();
    let after = run.report();
    let mut lists_back = BTreeMap::new();
    for (_, list) in after["session"]["edits_full"].as_object().unwrap() {
        lists_back.insert("one", list.clone());
    }
    assert_eq!(
        lists_back.get("one"),
        lists.as_object().unwrap().values().next(),
        "the edits and whose they were"
    );
    assert_eq!(after["app"]["note"], Value::Null, "it isn't reported as damaged");
    let lists = after["session"]["edits_full"].clone();
    // Killed again at once, with something on its way: still whole.
    run.ok(&["goto", "3"]);
    run.kill();
    let run = Run::on(&state).unwrap();
    run.settled();
    assert_eq!(run.report()["session"]["edits_full"], lists);
    run.quit();
}

/// The sleep file of the tab on screen, by the name the state's journals folder gives it, if it's there.
fn active_sleep_file(state: &State, report: &Value) -> Option<PathBuf> {
    let session = &report["session"];
    let tab = session["tabs"].as_array()?.iter().find(|tab| tab["id"] == session["active"])?;
    let file = &tab["file"];
    let stamp = recto_platform::FileStamp {
        size: file["size"].as_u64()?,
        mtime_ns: file["mtime_ns"].as_i64()?,
        device: file["device"].as_u64()?,
        inode: file["inode"].as_u64()?,
    };
    let name = recto_session::record::sleep_file_name(stamp, Path::new(tab["path"].as_str()?));
    Some(state.root.join("session").join("journals").join(name)).filter(|path| path.is_file())
}

/// Failure (review 1 of the slow mode): a flush right after opening tabs, as a quit or an update has it, skipped the tab on screen's sleep file whenever another was being written, with no trace, and the next launch showed bare paper for its first frame. It waits for the others for what's left of the flush's time, and past that the skip is counted. With sleep files' writes 1.5 s late, a flush straight after opening tabs leaves the tab on screen's file there or says it skipped it; and once the others are done, the next one writes it.
#[test]
fn a_flush_writes_the_tab_on_screens_sleep_file_or_says_it_skipped_it() {
    let state = State::new();
    let Some(run) = Run::on_with_env(&state, &[("RECTO_QA_SLOW", std::ffi::OsStr::new("sleep_write=1500"))]) else {
        return;
    };
    let folder = state.root.join("tabs");
    std::fs::create_dir_all(&folder).unwrap();
    let file = long_document("many", 3);
    for at in 0..8 {
        let link = folder.join(format!("t{at}.pdf"));
        recto_platform::local::clone_file(&file.0, &link).unwrap();
        run.ok(&["open", link.to_str().unwrap()]);
    }
    run.settled();
    run.ok(&["session", "save"]);
    let saved = run.report();
    let skipped = saved["record"]["sleep"]["writes_skipped_busy"].as_u64().unwrap();
    assert!(
        active_sleep_file(&state, &saved).is_some() || skipped >= 1,
        "the tab on screen's sleep file is there, or its skip is counted: {}",
        saved["record"]["sleep"]
    );
    // Given the time, it's written.
    let began = Instant::now();
    while active_sleep_file(&state, &run.report()).is_none() {
        assert!(began.elapsed() < Duration::from_secs(60), "never written: {}", run.report()["record"]["sleep"]);
        run.ok(&["session", "save"]);
    }
    run.quit();
}

/// Failure: a damaged record stops the app, panics, is overwritten, or is passed over in silence. It starts empty, says so, and what was there is kept to one side.
#[test]
fn a_damaged_record_starts_empty_and_says_so() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let file = long_document("damaged", 3);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.quit();
    let folder = state.root.join("session");
    for name in ["record.a.json", "record.b.json"] {
        if folder.join(name).exists() {
            std::fs::write(folder.join(name), b"{\"not\": \"a record\"}").unwrap();
        }
    }
    let run = Run::on(&state).unwrap();
    let after = run.report();
    assert_eq!(after["app"]["home"], true);
    assert!(after["app"]["note"].as_str().unwrap().contains("couldn't be read"), "{}", after["app"]["note"]);
    assert_eq!(
        std::fs::read(state.root.join("session.damaged/record.a.json")).unwrap(),
        b"{\"not\": \"a record\"}"
    );
}

/// Failure: a record from a newer version gets overwritten, so the update that wrote it finds it gone when it comes back. Left untouched, byte for byte, an empty start, and a notice.
#[test]
fn a_newer_record_is_left_alone() {
    let state = State::new();
    state.paths.prepare().unwrap();
    let folder = state.root.join("session");
    recto_platform::local::ensure_private_dir(&folder).unwrap();
    let newer = recto_session::record::seal_as(
        recto_session::record::MAIN_KIND,
        recto_session::record::CURRENT_VERSION + 1,
        9,
        b"from the future",
    );
    recto_platform::local::write_private_file(&folder.join("record.a.json"), &newer).unwrap();
    let Some(run) = Run::on(&state) else { return };
    let after = run.report();
    assert_eq!(after["app"]["home"], true);
    assert!(after["app"]["note"].as_str().unwrap().contains("newer"), "{}", after["app"]["note"]);
    let file = long_document("newer", 3);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.quit();
    assert_eq!(std::fs::read(folder.join("record.a.json")).unwrap(), newer, "still there, as it was");
    assert!(!folder.join("record.b.json").exists());
}

/// Failure: a file that's moved, changed or gone since the app last ran isn't said, or its edits go, or something's written to it. Each case, on a real file, with edits riding on it: the words are shown, the edits stay, and the file's bytes aren't touched. Moved, the tab follows it.
#[test]
fn what_became_of_a_file_is_said_and_its_edits_stay() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let folder = state.root.join("docs");
    std::fs::create_dir_all(folder.join("elsewhere")).unwrap();
    let bytes = pages(&(0..6).map(|at| Page::new(text_lines(&[&format!("Page {at}")]))).collect::<Vec<_>>());
    let (moved, changed, deleted) = (folder.join("moved.pdf"), folder.join("changed.pdf"), folder.join("deleted.pdf"));
    for path in [&moved, &changed, &deleted] {
        std::fs::write(path, &bytes).unwrap();
    }
    for path in [&moved, &changed, &deleted] {
        run.ok(&["open", path.to_str().unwrap()]);
        run.settled();
        run.ok(&["qa", "owner-edit", "rotate:2"]);
    }
    run.quit();
    let new_home = folder.join("elsewhere/renamed.pdf");
    std::fs::rename(&moved, &new_home).unwrap();
    let different = pages(&(0..2).map(|at| Page::new(text_lines(&[&format!("Another {at}")]))).collect::<Vec<_>>());
    std::thread::sleep(Duration::from_millis(5));
    std::fs::write(&changed, &different).unwrap();
    std::fs::remove_file(&deleted).unwrap();
    let was_moved = std::fs::read(&new_home).unwrap();
    let run = Run::on(&state).unwrap();
    // The look at each file is a thread's, once.
    let began = Instant::now();
    let show = loop {
        let show = run.ok(&["session", "show"]);
        if show["tabs"].as_array().unwrap().iter().filter(|tab| !tab["notice"].is_null()).count() == 3 {
            break show;
        }
        assert!(began.elapsed() < Duration::from_secs(10), "{show}");
        std::thread::sleep(Duration::from_millis(20));
    };
    let codes: Vec<&str> = show["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tab| tab["notice"]["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, ["file_moved", "file_changed", "file_missing"], "{show}");
    assert!(show["tabs"].as_array().unwrap().iter().all(|tab| tab["edited"] == true), "{show}");
    let session = run.report()["session"].clone();
    assert_eq!(session["tabs"][0]["path"], new_home.canonicalize().unwrap().to_str().unwrap(), "it followed");
    // The active tab says so where the reader will see it, and the sidebar does for all of them.
    let note = run.report()["app"]["note"].clone();
    assert!(note.as_str().unwrap().contains("can't be found"), "{note}");
    assert_eq!(std::fs::read(&new_home).unwrap(), was_moved, "nothing's written to it");
    assert_eq!(std::fs::read(&changed).unwrap(), different);
    assert!(!deleted.exists());
    let quiet = run.ok(&["sidebar"]);
    assert!(quiet["rows"].is_array());
}

/// Failure: reading writes to the disk all the time, wearing it for nothing: SPEC's budget. At rest, 0 bytes. Scrolling for a while, at most a line, as a minute hasn't passed. A burst of edits, about what the steps come to and not a rewrite per step. Going to the background writes what changed, and nothing after that when nothing has.
#[test]
fn it_writes_only_what_happened() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let file = long_document("wear", 60);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "20"]);
    run.settled();
    run.ok(&["qa", "frontmost", "off"]);
    std::thread::sleep(Duration::from_millis(600));
    let written = || process_counters(run.pid).unwrap().disk_bytes_written;
    // The OS charges a write when it reaches the disk. So what the commands above wrote, such as the activity log's lines, goes out first, and only what happens from here on is counted.
    let _ = Command::new("/bin/sync").status();
    let before = written();
    std::thread::sleep(Duration::from_secs(4));
    let idle = written() - before;
    assert_eq!(idle, 0, "at rest it writes nothing");
    let before = written();
    run.ok(&["qa", "sweep", "6", "--for", "3s"]);
    let scrolling = written() - before;
    eprintln!("3 s of scrolling wrote {scrolling} bytes, including the activity log's line");
    assert!(scrolling < 32 * 1024, "{scrolling}");
    // A scripted burst: each is a line of about 25 bytes, and they go out together in one append.
    let before = run.ok(&["session", "show"])["record"]["wrote_bytes"].as_u64().unwrap();
    for at in 0..50 {
        run.ok(&["qa", "owner-edit", &format!("rotate:{}", at % 60 + 1)]);
    }
    let began = Instant::now();
    let wrote = loop {
        let now = run.ok(&["session", "show"])["record"]["wrote_bytes"].as_u64().unwrap() - before;
        if now > 1000 {
            break now;
        }
        assert!(began.elapsed() < Duration::from_secs(20));
        std::thread::sleep(Duration::from_millis(100));
    };
    eprintln!("50 edits wrote {wrote} bytes to the journal");
    assert!(wrote < 50 * 40 + 1000, "{wrote}");
    run.quit();
}

/// Failure (review 1): with the state read late, after the window's gone up without it, the restore writes the saved "hidden" and "in the Dock" back into the record while the app's on the screen, so the next update hides an app the user's looking at. Once the host has shown it, that's how things are, in the record too.
#[test]
fn a_restore_after_the_window_is_up_keeps_what_the_host_did() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let file = long_document("alpha", 10);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "minimise", "on"]);
    run.ok(&["qa", "hide", "on"]);
    let saved = run.report();
    assert_eq!(
        (&saved["session"]["app"]["app_hidden"], &saved["session"]["app"]["window"]["minimised"]),
        (&json!(true), &json!(true))
    );
    run.quit();
    // A slow disk: the state comes 2.5 s after the launch, past the window's patience.
    let app = app_path().unwrap();
    let output = state
        .command()
        .env("RECTO_QA_RESTORE_DELAY_MS", "2500")
        .args(["qa", "start", "--app", app.to_str().unwrap(), "--time-limit", "2m", "--json"])
        .output()
        .unwrap();
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    let run = Run::attach(&state, reply["data"]["pid"].as_u64().unwrap() as u32);
    let restored = common::app::wait_for_long(&run, |report| report["session"]["tabs"].as_array().is_some_and(|tabs| tabs.len() == 1));
    assert_eq!(restored["app"]["hidden"], false, "the host showed it");
    assert_eq!(restored["app"]["window"]["minimised"], false);
    assert_eq!(restored["session"]["app"]["app_hidden"], false, "and the record says so");
    assert_eq!(restored["session"]["app"]["window"]["minimised"], false);
    run.quit();
}

/// Failure: a session with no window has its save land on the app's record, losing the owner's tabs, or the commands don't exist without an app. It has a folder of its own, and comes back.
#[test]
fn a_session_with_no_window_has_its_own_record() {
    let state = State::at(Level::Full);
    let _serving = state.serve();
    let file = long_document("headless", 6);
    let (reply, code) = state.json(&["open", file.0.to_str().unwrap()]);
    assert_eq!(code, 0, "{reply}");
    state.json(&["page", "rotate", "2", "--by", "90"]);
    state.json(&["goto", "4"]);
    let (saved, code) = state.json(&["session", "save"]);
    assert_eq!(code, 0, "{saved}");
    assert_eq!(saved["data"]["saved"], true);
    state.json(&["goto", "1"]);
    state.json(&["discard"]);
    let (restored, code) = state.json(&["session", "restore"]);
    assert_eq!(code, 0, "{restored}");
    assert_eq!(restored["data"]["restored"], true);
    let (place, _) = state.json(&["where"]);
    assert_eq!(place["data"]["page"], 4);
    assert_eq!(place["data"]["edited"], true);
    assert!(state.root.join("session-windowless").exists());
    assert!(!state.root.join("session").exists(), "the app's isn't touched");
    let _ = state.json(&["discard"]);
}

/// Failure (review T11): in a session with no window a discard doesn't reach the journal, so restoring brings back what was discarded, as a second tab. Saving after it takes the file away.
#[test]
fn a_discard_stays_discarded_in_a_session_with_no_window() {
    let state = State::at(Level::Full);
    let _serving = state.serve();
    let file = long_document("discarded", 6);
    state.json(&["open", file.0.to_str().unwrap()]);
    state.json(&["page", "rotate", "2", "--by", "90"]);
    state.json(&["session", "save"]);
    state.json(&["discard"]);
    state.json(&["session", "save"]);
    let (restored, code) = state.json(&["session", "restore"]);
    assert_eq!(code, 0, "{restored}");
    assert_eq!(restored["data"]["count"], 1, "{restored}");
    let (place, _) = state.json(&["where"]);
    assert_eq!(place["data"]["edited"], false);
}

// The real update, whole, with this file's own comparison.
#[path = "restore/update.rs"]
mod update;

#[path = "restore/ci_known.rs"]
mod ci_known;
#[path = "restore/exits.rs"]
mod exits;
#[path = "restore/many.rs"]
mod many;

#[path = "restore/slow_stage.rs"]
mod slow_stage;

/// Failure: a note's bubble keeps the shape it had for the margin when it was laid out, so after a zoom changes the margin it stays as narrow as it was zoomed in, until something unrelated, a change of appearance, lays it out again. A file notice wrapped to 5 lines in light and 2 in dark (2026-11-08). Its shape follows the margin there is now, and is the same in light and dark.
#[test]
fn a_notes_bubble_follows_the_margin_and_not_the_appearance() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let file = text_document(&folder, "changing.pdf", 4);
    run.ok(&["qa", "appearance", "light"]);
    run.ok(&["open", file.to_str().unwrap()]);
    run.settled();
    run.quit();
    std::thread::sleep(Duration::from_millis(5));
    text_document(&folder, "changing.pdf", 6);
    let run = Run::on(&state).unwrap();
    run.ok(&["qa", "appearance", "light"]);
    // Small, so the margin is wide and the note takes fewer lines than it does zoomed in.
    run.ok(&["view", "--zoom", "40%"]);
    run.settled();
    let bubble = |state: &Value| state["app"]["note_box"].clone();
    wait_until("the note comes", || {
        run.report()["app"]["note"].as_str().is_some_and(|note| note.contains("changed"))
    });
    let before = bubble(&run.report());
    let width = |shape: &Value| shape["width"].as_f64().unwrap_or_else(|| panic!("no bubble: {shape}"));
    run.ok(&["view", "--zoom", "300%"]);
    run.settled();
    let zoomed = bubble(&run.report());
    assert!(
        width(&zoomed) < width(&before) - 20.0 && zoomed["lines"].as_u64() > before["lines"].as_u64(),
        "zoomed in, the margin is narrower, so the bubble is: {before} then {zoomed}"
    );
    run.ok(&["view", "--zoom", "40%"]);
    run.settled();
    let light = bubble(&run.report());
    assert!(
        (width(&light) - width(&before)).abs() < 0.5 && light["lines"] == before["lines"],
        "back at the same zoom, the bubble is as it was: {before} then {light}"
    );
    run.ok(&["qa", "appearance", "dark"]);
    run.settled();
    let dark = run.report();
    assert_eq!(dark["app"]["appearance"], "dark");
    let dark = bubble(&dark);
    assert!(
        (width(&dark) - width(&light)).abs() < 0.5 && dark["lines"] == light["lines"],
        "the same note is the same shape in light and dark: {light} and {dark}"
    );
}

/// Failure (CI, build/privacy, run 43062872948: `field_has_focus: true then false` across an update): the session thread told the host the state was in and went on without bringing Find's field up to date, which every other call does as it ends. The window made then had Find closed for all it knew, so its field didn't take the keyboard it had had, though the focus was said to be on it; on a machine where nothing else came first, as a starved one, every time. The session thread 1.5 s late after the state comes in, and the field has the keyboard again.
#[test]
fn finds_field_has_the_keyboard_again_when_the_session_is_late_after_the_state() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let file = long_document("alpha", 5);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "menu", "find.open"]);
    run.ok(&["qa", "find-type", "words"]);
    run.settled();
    assert_eq!(run.report()["app"]["find"]["field_has_focus"], true);
    run.quit();
    let run = Run::on_with_env(&state, &[("RECTO_QA_SLOW", std::ffi::OsStr::new("session=1500"))]).unwrap();
    run.settled();
    let after = run.report();
    assert_eq!(after["app"]["find"]["open"], true, "{}", after["app"]["find"]);
    assert_eq!(after["app"]["focus_view"], "find");
    assert_eq!(
        after["app"]["find"]["field_has_focus"], true,
        "the field has the keyboard: {}",
        after["app"]["find"]
    );
    run.quit();
}

/// Failure: a protected tab comes back after a quit open, as though its password had been kept; or it comes back saying nothing, with no way to type it again; or Cancel leaves it asking at once.
#[test]
fn a_protected_tab_asks_again_after_a_quit() {
    protected_tab_asks_again(&[]);
}

/// Failure (slow mode, 2026-11-11): with the state read late, the password sheet went up on the window that was up without it, and that window went for one made from the state. Cancel on the sheet went to the old window's ending, after it had gone, so it was never answered: the sheet came straight back, and stayed counted open, so `session check` would say a sheet's in the way of every hand-off. The state read 1.5 s late and the window made again 300 ms after it: Cancel holds, and nothing's left counted.
#[test]
fn a_protected_tab_asks_again_when_the_state_is_read_late() {
    protected_tab_asks_again(&[
        ("RECTO_QA_RESTORE_DELAY_MS", std::ffi::OsStr::new("1500")),
        ("RECTO_QA_REMAKE_DELAY_MS", std::ffi::OsStr::new("300")),
    ]);
}

/// A protected tab, a quit, and a launch with this in its environment: asked again, and Cancel holds.
fn protected_tab_asks_again(second_launch: &[(&str, &std::ffi::OsStr)]) {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let locked = state.root.join("locked.pdf");
    std::fs::write(&locked, common::locked_with("open sesame", 1)).unwrap();
    run.ok(&["qa", "open-file", locked.to_str().unwrap()]);
    let until = |run: &Run, holds: &dyn Fn(&Value) -> bool| {
        for _ in 0..500 {
            let state = run.report();
            if holds(&state) {
                return state;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("it never came to pass: {}", run.report());
    };
    until(&run, &|state| state["app"]["sheet"]["kind"] == "password");
    let (reply, code) = state.json_with_stdin(&["qa", "password"], "open sesame");
    assert_eq!(code, 0, "{reply}");
    until(&run, &|state| {
        state["app"]["sheet"] == json!({}) && state["session"]["tabs"].as_array().is_some_and(|tabs| tabs.len() == 1)
    });
    run.settled();
    run.quit();
    let run = Run::on_with_env(&state, second_launch).unwrap();
    let asked = until(&run, &|state| state["app"]["sheet"]["kind"] == "password");
    assert!(asked["app"]["sheet"]["informative"].as_str().unwrap().contains("asked again"), "{asked}");
    run.ok(&["qa", "sheet-answer", "second"]);
    let declined = until(&run, &|state| state["app"]["sheet"] == json!({}));
    assert!(
        declined["app"]["note"].as_str().is_some_and(|note| note.contains("password again")),
        "{declined}"
    );
    let check = run.ok(&["session", "check"]);
    assert_eq!(check["safe"], true, "no sheet's left counted open: {check}");
    run.quit();
}
