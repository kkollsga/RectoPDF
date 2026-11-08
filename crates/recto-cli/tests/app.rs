//! The real app in a quiet run, through `recto`, as an agent reaches it. Off every screen, never active, on temp state: the owner keeps working while these run.
//!
//! The app has to be built, and these skip if it isn't, so `tools/check.sh --quick` stays fast and works for anyone without it. `--full` builds it and sets `RECTO_REQUIRE_APP`, so there a missing app is a failure. Asserts are on what the app reports. Pictures are for people.

mod common;

use common::{State, bin};
use recto_agent::{Level, exit};
use recto_testpdf::{Object, Page, TempPdf, build, form_bomb, hidden_text_page, pages, text_lines};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Command;

fn app_path() -> Option<PathBuf> {
    if std::env::var_os("RECTO_SKIP_APP").is_some() {
        return None;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app/build/RectoPDF.app/Contents/MacOS/RectoPDF");
    if root.is_file() {
        return Some(root);
    }
    assert!(std::env::var_os("RECTO_REQUIRE_APP").is_none(), "the app isn't built, and this run needs it");
    eprintln!("skipped: the app isn't built");
    None
}

/// A quiet run, ended on drop.
struct Run {
    state: State,
    pid: u32,
}

impl Run {
    fn start() -> Option<Self> {
        Self::start_with(&[])
    }

    fn start_with(extra: &[&str]) -> Option<Self> {
        let app = app_path()?;
        let state = State::new();
        let mut args = vec!["qa", "start", "--app", app.to_str().unwrap(), "--time-limit", "2m"];
        args.extend_from_slice(extra);
        let (reply, code) = state.json(&args);
        assert_eq!(code, 0, "{reply}");
        let pid = reply["data"]["pid"].as_u64().unwrap() as u32;
        Some(Self { state, pid })
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (reply, code) = self.state.json(args);
        assert_eq!((code, &reply["ok"]), (0, &json!(true)), "{args:?}: {reply}");
        reply["data"].clone()
    }

    fn settled(&self) {
        self.ok(&["qa", "wait-settled", "--timeout", "30s"]);
    }

    fn report(&self) -> Value {
        self.ok(&["qa", "state"])
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        let _ = self.state.json(&["stop"]);
        // A run that's still here after that isn't to be left on the owner's Mac.
        for _ in 0..100 {
            if still_running(self.pid) {
                std::thread::sleep(std::time::Duration::from_millis(20));
            } else {
                return;
            }
        }
        let _ = Command::new("/bin/kill").arg("-9").arg(self.pid.to_string()).status();
    }
}

fn still_running(pid: u32) -> bool {
    recto_platform::local::process_counters(pid).is_some()
}

fn long_document(count: usize) -> TempPdf {
    let list: Vec<Page> = (0..count).map(|index| Page::new(text_lines(&[&format!("Page number {index}"), "Words, so that the page isn't blank"]))).collect();
    TempPdf::new("long.pdf", &pages(&list))
}

/// Two pages with an outline, so the sidebar has contents.
fn outlined() -> TempPdf {
    let page = |content: usize| Object::Dict(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {content} 0 R >>"));
    let content = |text: &str| Object::Stream { dict: String::new(), data: format!("BT /F1 12 Tf 72 700 Td ({text}) Tj ET").into_bytes() };
    let objects = vec![
        Object::Dict("<< /Type /Catalog /Pages 2 0 R /Outlines 3 0 R >>".into()),
        Object::Dict("<< /Type /Pages /Kids [6 0 R 7 0 R] /Count 2 >>".into()),
        Object::Dict("<< /Type /Outlines /First 4 0 R /Last 5 0 R /Count 2 >>".into()),
        Object::Dict("<< /Title (One) /Parent 3 0 R /Next 5 0 R /Dest [6 0 R /Fit] >>".into()),
        Object::Dict("<< /Title (Two) /Parent 3 0 R /Prev 4 0 R /Dest [7 0 R /Fit] >>".into()),
        page(8),
        page(9),
        content("First"),
        content("Second"),
    ];
    TempPdf::new("outlined.pdf", &build(&objects))
}

/// Failure: a QA run takes the screen, the focus or the Dock from the person using this Mac, or shows anything on a display. Every claim a quiet run makes, as the app itself reports it.
#[test]
fn a_quiet_run_stays_out_of_the_way() {
    let Some(run) = Run::start() else { return };
    let app = run.report()["app"].clone();
    assert_eq!(app["is_active"], false, "{app}");
    assert_eq!(app["activation"], "prohibited", "no Dock icon, no menu bar: {app}");
    assert_eq!(app["window"]["is_visible"], false, "never ordered in: {app}");
    assert_eq!(app["window"]["on_any_screen"], false, "{app}");
    assert_eq!(app["home"], true, "no PDF open, so the home state: {app}");
    assert_eq!(app["agent_icon"], true);
    let file = long_document(4);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let later = run.report()["app"].clone();
    assert_eq!((later["is_active"].clone(), later["activation"].clone(), later["window"]["is_visible"].clone()), (json!(false), json!("prohibited"), json!(false)), "drawing a page changes none of that: {later}");
}

/// Failure: the app and `recto` have a session each, so an agent sees nothing the reader does, or moves nothing the reader sees. One session, in both directions, over the real socket.
#[test]
fn recto_acts_on_the_running_app_and_sees_what_it_does() {
    let Some(run) = Run::start() else { return };
    let file = long_document(30);
    let opened = run.ok(&["open", file.0.to_str().unwrap()]);
    assert_eq!(opened["pages"], 30);
    run.settled();
    assert_eq!(run.ok(&["status"])["tabs"], 1);
    assert_eq!(run.report()["app"]["home"], false, "the page view is showing");
    // An agent moves the reader.
    run.ok(&["goto", "12"]);
    run.settled();
    let state = run.report();
    assert!(state["reader"]["visible_pages"].as_array().unwrap().contains(&json!(12)), "{state}");
    assert_eq!(run.ok(&["where"])["page"], 12, "and doesn't get moved back: {state}");
    // The reader moves, and an agent can see it.
    let height = state["reader"]["document_height"].as_f64().unwrap() / 30.0;
    run.ok(&["qa", "scroll", &format!("{}", height * 5.0)]);
    run.settled();
    let now = run.report();
    let page = now["reader"]["page"].as_u64().unwrap();
    assert!((16..=18).contains(&page), "{now}");
    // The session thread follows a moment later. `recto wait` is what an agent would use.
    let mut followed = 0;
    for _ in 0..200 {
        followed = run.ok(&["where"])["page"].as_u64().unwrap();
        if followed == page {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(followed, page);
    run.ok(&["goto", "1"]);
    run.settled();
    run.ok(&["page", "delete", "1"]);
    run.settled();
    let edited = run.report();
    assert_eq!(edited["reader"]["pages"], 29, "an edit reaches the page view: {edited}");
}

/// Failure: the app's session has no indexer, or the bundle has no helper, so a search in the app never has an index, and `recto wait indexing-done` waits out its time on something nobody can start. The CLI's own session passes all of this while the app's doesn't.
#[test]
fn a_search_in_the_app_gets_its_index() {
    let Some(run) = Run::start() else { return };
    let file = long_document(4);
    run.ok(&["open", file.0.to_str().unwrap()]);
    let done = run.ok(&["wait", "indexing-done", "--tab", "active", "--timeout", "60s"]);
    assert_eq!(done["state"], "ready", "{done}");
    let found = run.ok(&["search", "number"]);
    assert_eq!(found["route"], "index", "{found}");
    assert_eq!(found["hits"].as_array().unwrap().len(), 4, "{found}");
}

/// Failure: tapping the active PDF switches tabs or does nothing, tapping another folds this one, or a contents row doesn't go to its page. The sidebar's rules, by what a click calls, and read back from what's drawn.
#[test]
fn the_sidebar_folds_switches_and_goes_to_contents() {
    let Some(run) = Run::start() else { return };
    let (first, second) = (outlined(), long_document(2));
    run.ok(&["open", first.0.to_str().unwrap()]);
    run.settled();
    let open = run.report()["app"]["sidebar_rows"].as_u64().unwrap();
    assert_eq!(open, 3, "the tab and its two contents rows");
    // A contents row goes to its page.
    run.ok(&["qa", "click-row", "2"]);
    wait_for(&run, |state| state["reader"]["visible_pages"].as_array().unwrap().contains(&json!(2)));
    // Tapping the active tab folds its contents, and stays on it.
    run.ok(&["qa", "click-row", "0"]);
    let folded = wait_for(&run, |state| state["app"]["sidebar_rows"] == 1);
    assert_eq!(folded["app"]["home"], false);
    run.ok(&["qa", "click-row", "0"]);
    wait_for(&run, |state| state["app"]["sidebar_rows"] == 3);
    // Tapping another tab switches to it, and each keeps its own fold.
    run.ok(&["open", second.0.to_str().unwrap()]);
    run.settled();
    assert_eq!(run.ok(&["where"])["tab"], 2);
    run.ok(&["qa", "click-row", "0"]);
    wait_for(&run, |state| state["reader"]["tab"] == 1);
    // Details come with hover, and go with it.
    assert_eq!(run.ok(&["qa", "hover", "1"])["hover"], 1);
    assert!(run.ok(&["qa", "hover", "none"])["hover"].is_null());
}

fn wait_for(run: &Run, holds: impl Fn(&Value) -> bool) -> Value {
    let mut last = Value::Null;
    for _ in 0..400 {
        last = run.report();
        if holds(&last) {
            return last;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("it never came to pass: {last}");
}

/// Failure: a QA command works below its level, or a read needs more than Read-only, so the level says nothing about a run an agent drives. Over the real socket, at each level.
#[test]
fn qa_commands_keep_to_their_levels() {
    let Some(run) = Run::start() else { return };
    let out = run.state.root.join("level.png");
    run.state.set_level(Level::Off);
    for args in [vec!["qa", "state"], vec!["qa", "zoom", "1.5"]] {
        assert_eq!(run.state.json(&args).1, exit::PERMISSION, "{args:?} at Off");
    }
    run.state.set_level(Level::ReadOnly);
    assert_eq!(run.state.json(&["qa", "state"]).1, 0, "reading is reading");
    assert_eq!(run.state.json(&["qa", "shot", "--out", out.to_str().unwrap()]).1, 0, "so is a picture");
    assert_eq!(run.state.json(&["qa", "zoom", "1.5"]).1, exit::PERMISSION, "changing isn't");
    run.state.set_level(Level::Control);
    assert_eq!(run.state.json(&["qa", "zoom", "1.5"]).1, 0);
    run.state.set_level(Level::Full);
}

/// Failure: a QA command goes through on a session with no app, or on a normal one, so anything can drive the app. `recto serve` holds a session with no QA in it, as a shipped app does.
#[test]
fn qa_commands_are_refused_without_a_quiet_app() {
    let state = State::at(Level::Full);
    let _serving = state.serve();
    let (reply, code) = state.json(&["qa", "state"]);
    assert_eq!((reply["error"]["code"].as_str(), code), (Some("qa_needs_app"), exit::FAILED));
    let (reply, _) = state.json(&["qa", "zoom", "1.5"]);
    assert_eq!(reply["error"]["code"], "qa_needs_app");
}

/// Failure: a QA run can be pointed at the owner's real state, or the app starts one with no state of its own. Refused before anything's started.
#[test]
fn a_quiet_run_refuses_the_real_state() {
    let Some(app) = app_path() else { return };
    let output = Command::new(bin()).env_remove(recto_control::state::STATE_ENV).args(["qa", "start", "--app", app.to_str().unwrap(), "--json"]).output().unwrap();
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reply["error"]["code"], "needs_temp_state", "{reply}");
    // The app itself says the same, whoever starts it.
    let output = Command::new(&app).arg("--qa").env_remove(recto_control::state::STATE_ENV).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("RECTO_STATE_DIR"), "{}", String::from_utf8_lossy(&output.stderr));
}

/// Failure: a stall on the UI thread passes unseen, so hangs ship. A stall of 2 s fails the script, and one a good deal shorter doesn't.
#[test]
fn the_watchdog_catches_a_stall_and_it_fails_the_script() {
    let Some(run) = Run::start() else { return };
    let script = run.state.root.join("short.qa");
    std::fs::write(&script, "# A short pause isn't a stall.\nqa stall 300\nqa state\nexpect hangs.over_2s 0\n").unwrap();
    assert_eq!(run.state.json(&["qa", "run", script.to_str().unwrap()]).1, 0);
    let long = run.state.root.join("long.qa");
    std::fs::write(&long, "qa stall 2300\n").unwrap();
    let (reply, code) = run.state.json(&["qa", "run", long.to_str().unwrap()]);
    assert_eq!(code, exit::FAILED, "{reply}");
    assert!(reply["error"]["message"].as_str().unwrap().contains("stalled"), "{reply}");
    let hangs = run.report()["hangs"].clone();
    assert_eq!(hangs["over_2s"], 1, "{hangs}");
    assert!(hangs["worst_ms"].as_u64().unwrap() >= 2000, "{hangs}");
}

/// Failure: a shot needs the screen or a permission, comes out at the wrong size, or is blank. Run quietly, so a screen isn't involved. The picture must have ink in it: a real PNG, of the window at 2x, and larger than the same window with no page.
#[test]
fn a_shot_is_a_real_picture_of_the_window_at_2x() {
    let Some(run) = Run::start() else { return };
    let empty = run.state.root.join("empty.png");
    run.ok(&["qa", "resize", "1000x700"]);
    let before = run.ok(&["qa", "shot", "--out", empty.to_str().unwrap()]);
    assert_eq!((before["width"].as_u64(), before["height"].as_u64()), (Some(2000), Some(1400)), "{before}");
    let file = long_document(2);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let shot = run.state.root.join("page.png");
    let with_page = run.ok(&["qa", "shot", "--out", shot.to_str().unwrap()]);
    let bytes = std::fs::read(&shot).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    assert_ne!(with_page["bytes"], before["bytes"], "a page changes the picture");
}

/// Failure: the appearance, paper or the sidebar don't reach what's drawn, or a change draws nothing new. Read back from the app and from the tiles it attached. The sidebar goes by its own button, and hiding it leaves the page, its number, the agent icon and the button to bring it back.
#[test]
fn dark_mode_paper_and_the_sidebar_change_what_is_drawn() {
    let Some(run) = Run::start() else { return };
    let file = long_document(3);
    run.ok(&["qa", "appearance", "light"]);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    assert_eq!(run.report()["app"]["paper"], "white");
    let drawn = run.report()["reader"]["counts"]["drawn"].as_u64().unwrap();
    assert_eq!(run.ok(&["qa", "appearance", "dark"])["appearance"], "dark");
    let dark = wait_for(&run, |state| state["reader"]["paper"] == "dark");
    assert!(dark["reader"]["counts"]["drawn"].as_u64().unwrap() >= drawn);
    run.settled();
    assert!(run.report()["reader"]["counts"]["drawn"].as_u64().unwrap() > drawn, "dark paper is drawn anew");
    assert_eq!(run.ok(&["view", "--paper", "white"])["paper"], "white");
    wait_for(&run, |state| state["reader"]["paper"] == "white");
    run.ok(&["view", "--paper", "auto"]);
    run.settled();
    let shown = run.report();
    assert_eq!(shown["app"]["sidebar_button"]["visible"], true, "{shown}");
    assert_eq!(shown["app"]["sidebar_button"]["clear_of_window_buttons"], true, "{shown}");
    run.ok(&["qa", "button", "sidebar"]);
    let hidden = wait_for(&run, |state| state["app"]["sidebar_hidden"] == true);
    assert_eq!(hidden["app"]["sidebar_visible"], false, "{hidden}");
    assert_eq!(hidden["app"]["agent_icon"], true, "kept");
    assert_eq!(hidden["app"]["sidebar_button"]["visible"], true, "the way back stays: {hidden}");
    assert_eq!(hidden["app"]["window_buttons_shown"], true, "nothing else goes with it: {hidden}");
    assert!(hidden["app"]["place_label"].as_str().is_some_and(|place| place.contains("of 3")), "the page number stays: {hidden}");
    assert_eq!(run.ok(&["view"])["sidebar"], false, "the same setting `recto view` has");
    run.ok(&["qa", "menu", "view.sidebar"]);
    wait_for(&run, |state| state["app"]["sidebar_visible"] == true);
    run.ok(&["view", "--sidebar", "off"]);
    wait_for(&run, |state| state["app"]["sidebar_visible"] == false);
    run.ok(&["view", "--sidebar", "on"]);
    wait_for(&run, |state| state["app"]["sidebar_visible"] == true);
}

/// Failure: the page number isn't in the lower right with the sidebar showing, or goes with it hidden, or doesn't follow the page. Owner, 2026-11-07: "The page number should always be visible in the lower right."
#[test]
fn the_page_number_is_always_in_the_lower_right() {
    let Some(run) = Run::start() else { return };
    let file = long_document(5);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let first = run.report();
    assert_eq!((&first["app"]["place_label"], &first["app"]["place_label_in_lower_right"]), (&json!("1 of 5"), &json!(true)), "{first}");
    run.ok(&["goto", "4"]);
    wait_for(&run, |state| state["app"]["place_label"] == "4 of 5");
    run.ok(&["view", "--sidebar", "off"]);
    let hidden = wait_for(&run, |state| state["app"]["sidebar_visible"] == false);
    assert_eq!((&hidden["app"]["place_label"], &hidden["app"]["place_label_in_lower_right"]), (&json!("4 of 5"), &json!(true)), "{hidden}");
    run.ok(&["qa", "resize", "700x500"]);
    let narrow = run.report();
    assert_eq!(narrow["app"]["place_label_in_lower_right"], true, "in a small window too: {narrow}");
}

/// Failure: the right-click menu doesn't offer the layouts or tick the one showing; side by side doesn't put two pages to a row, loses the reader's place, or leaves a click or a drag on the right-hand page landing on the left one; zoom or the page number stop working in it; or one tab's layout changes another's. Owner, 2026-11-07: "In the right click menu you can select side by side layout, with and without header page."
#[test]
fn side_by_side_lays_out_selects_and_keeps_the_place() {
    let Some(run) = Run::start() else { return };
    let (file, other) = (long_document(9), TempPdf::new("other.pdf", &pages(&[Page::new(text_lines(&["Another document"]))])));
    run.ok(&["open", other.0.to_str().unwrap()]);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let single = run.report();
    let layouts = |state: &Value| -> Vec<(String, bool)> { state["app"]["context_menu"].as_array().unwrap().iter().filter(|item| item["id"].as_str().unwrap().starts_with("layout.")).map(|item| (item["id"].as_str().unwrap().to_string(), item["checked"] == true)).collect() };
    assert_eq!(layouts(&single), [("layout.single".to_string(), true), ("layout.two_up".to_string(), false), ("layout.two_up_cover".to_string(), false)], "{single}");
    run.ok(&["goto", "5"]);
    run.settled();
    let single_height = run.report()["reader"]["document_height"].as_f64().unwrap();
    run.ok(&["qa", "context-menu", "layout.two_up"]);
    wait_for(&run, |state| state["app"]["layout"] == "two-up");
    run.settled();
    let two = run.report();
    assert!(layouts(&two)[1].1, "the one showing is ticked: {two}");
    assert!(two["reader"]["document_height"].as_f64().unwrap() < single_height * 0.7, "two to a row: {two}");
    assert!(two["reader"]["visible_pages"].as_array().unwrap().contains(&json!(5)), "still on page 5: {two}");
    assert!(two["app"]["place_label"].as_str().is_some_and(|place| place.ends_with("of 9")), "{two}");
    assert_eq!(run.ok(&["view"])["layout"], "two-up", "the same setting `recto view` has");
    // A double click on the right-hand page's first word selects on that page, and a drag from the left page to the right takes both, in order.
    run.ok(&["goto", "1"]);
    run.settled();
    run.ok(&["qa", "mouse-down", &format!("{}:2", place_on(&run, 2, 0, 0.3))]);
    run.ok(&["qa", "mouse-up"]);
    run.settled();
    let word = run.report();
    assert_eq!(word["reader"]["selection"]["start"]["page"], 2, "{word}");
    run.ok(&["qa", "mouse-down", &place_on(&run, 1, 0, 0.0)]);
    run.ok(&["qa", "mouse-drag", &place_on(&run, 2, 0, 1.0)]);
    run.ok(&["qa", "mouse-up"]);
    run.settled();
    copy_now(&run);
    let text = copied(&run);
    assert!(text.starts_with("Page number 0") && text.contains("Page number 1"), "{text:?}");
    assert_eq!(run.ok(&["view", "--zoom", "150%"])["zoom_level"], "150%", "zoom works side by side");
    run.ok(&["qa", "context-menu", "layout.two_up_cover"]);
    wait_for(&run, |state| state["app"]["layout"] == "two-up-cover" && state["reader"]["spread"] == "TwoUpCover");
    // Each tab its own.
    run.ok(&["tap", "#1"]);
    let first = wait_for(&run, |state| state["app"]["layout"] == "single");
    assert_eq!(first["reader"]["spread"], "Single", "{first}");
    run.ok(&["tap", "#2"]);
    wait_for(&run, |state| state["app"]["layout"] == "two-up-cover");
    run.ok(&["view", "--layout", "single"]);
    wait_for(&run, |state| state["reader"]["spread"] == "Single");
    let (reply, code) = run.state.json(&["view", "--layout", "three-up"]);
    assert_eq!(code, exit::USAGE, "{reply}");
}

/// Failure: the panel's Rotate can be pressed with nothing to turn, or turns one page, or takes a step a page so one undo doesn't take it back, or isn't drawn turned, or doesn't show as unsaved in the red button's dot, or Undo doesn't take it back with the window key, where NSWindow took it first (review of f05c998, B1); or what was drawn is dropped for a turn, which changes nothing of a page's pixels, so turning back draws it all again. Owner, 2026-11-07: "When not in edit mode rotate page will rotate all pages together (it still enables save to commit the adjustment)".
#[test]
fn the_panels_rotate_turns_every_page_and_undoes_in_one() {
    let Some(run) = Run::start() else { return };
    // With nothing open there's nothing to turn, so Rotate is greyed out rather than doing nothing.
    run.ok(&["qa", "panel", "toggle"]);
    let home = run.report();
    assert!(home["app"]["panel"]["items"].as_array().unwrap().iter().filter(|item| item["id"].as_str().unwrap().starts_with("rotate.")).all(|item| item["enabled"] == false), "{}", home["app"]["panel"]);
    run.ok(&["qa", "panel", "toggle"]);
    let file = long_document(4);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let before = run.report();
    assert_eq!(before["app"]["document_edited"], false);
    run.ok(&["qa", "panel", "toggle"]);
    let open = run.report();
    assert!(open["app"]["panel"]["items"].as_array().unwrap().iter().any(|item| item["id"] == "rotate.right"), "{open}");
    run.ok(&["qa", "panel", "rotate.right"]);
    wait_for(&run, |state| state["app"]["document_edited"] == true);
    run.settled();
    let turned = run.report();
    let drawn = turned["reader"]["counts"]["drawn"].as_u64().unwrap();
    assert!(turned["app"]["pages"]["page_turns"].as_array().unwrap().iter().all(|turn| turn == 1), "every page shows turned: {}", turned["app"]["pages"]);
    assert!(turned["app"]["pages"]["page_frames_landscape"].as_u64().unwrap() >= 1, "and wider than tall: {}", turned["app"]["pages"]);
    let edits = run.ok(&["edits"]);
    assert_eq!(edits["can_undo"], true, "{edits}");
    run.ok(&["qa", "key-action", "undo"]);
    wait_for(&run, |state| state["app"]["document_edited"] == false);
    run.settled();
    let back = run.report();
    assert!(back["app"]["pages"]["page_turns"].as_array().unwrap().iter().all(|turn| turn == 0), "one undo takes every page back: {}", back["app"]["pages"]);
    assert_eq!(run.ok(&["edits"])["can_undo"], false, "it was one step");
    // A landscape page fits at another scale, so a turn draws at that scale; what was drawn before isn't dropped, so turning back draws nothing new.
    assert!(back["reader"]["counts"]["drawn"].as_u64().unwrap() <= drawn + 1, "turning back drew again: {drawn} then {}", back["reader"]["counts"]["drawn"]);
    run.ok(&["qa", "key-action", "redo"]);
    wait_for(&run, |state| state["app"]["document_edited"] == true);
    // An agent's way, the same method.
    run.ok(&["qa", "key-action", "undo"]);
    wait_for(&run, |state| state["app"]["document_edited"] == false);
    run.ok(&["page", "rotate", "all", "--by", "180"]);
    let half = wait_for(&run, |state| state["app"]["pages"]["page_turns"].as_array().is_some_and(|turns| !turns.is_empty() && turns.iter().all(|turn| turn == 2)));
    assert_eq!(half["app"]["document_edited"], true);
}

/// Failure: the zoom level isn't in the top right just left of the agent icon, in its row, or doesn't take the icon's place at the same margin when the icon's hidden, or the page's scroller runs up beside it; or it says a level the page isn't at, or clicking it doesn't let the reader type a level or fit the page again. Owner, 2026-11-07: "clicking the zoom value enables you to type in zoom level or reset", and 2026-11-08: "The zoom level should be in the upper right of the app (not in the sidebar)". 100% is the page's real size.
#[test]
fn the_zoom_level_shows_and_takes_a_typed_level() {
    let Some(run) = Run::start() else { return };
    let file = long_document(5);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let magnification = |state: &Value| state["app"]["pages"]["magnification"].as_f64().unwrap();
    let rest = run.report();
    let level = &rest["app"]["zoom_level"];
    assert_eq!((&level["visible"], &level["editing"], &level["in_the_top_right"], &level["clear_of_agent_icon"], &level["in_the_agent_icons_row"], &level["above_the_scroller"]), (&json!(true), &json!(false), &json!(true), &json!(true), &json!(true), &json!(true)), "{rest}");
    let fit = rest["reader"]["fit"].as_f64().unwrap();
    assert_eq!(level["text"], format!("{:.0}%", (magnification(&rest) * fit * 100.0).round()), "it says the level the page is at: {rest}");
    let open = run.ok(&["qa", "button", "zoom"]);
    assert_eq!(open["zoom_level"]["editing"], true, "{open}");
    assert!(open["zoom_level"]["field"].as_str().is_some_and(|text| text.parse::<f64>().is_ok()), "the number alone, to type over: {open}");
    assert!(open["zoom_level"]["reset"].is_string(), "{open}");
    assert_eq!((&open["zoom_level"]["reset_clear_of_field"], &open["zoom_level"]["above_the_scroller"]), (&json!(true), &json!(true)), "the field opens leftwards, Fit before it: {open}");
    run.ok(&["qa", "zoom-type", "200"]);
    let at_200 = wait_for(&run, |state| state["app"]["zoom_level"]["text"] == "200%");
    assert!((magnification(&at_200) * fit - 2.0).abs() < 0.01, "200% is twice the real size: {at_200}");
    assert_eq!(at_200["app"]["zoom_level"]["editing"], false);
    run.ok(&["qa", "zoom-type", "banana"]);
    run.settled();
    let same = run.report();
    assert_eq!(same["app"]["zoom_level"]["text"], "200%", "what isn't a level changes nothing: {same}");
    assert_eq!(same["app"]["zoom_level"]["editing"], false);
    run.ok(&["qa", "zoom-type", "100000"]);
    let most = wait_for(&run, |state| state["app"]["zoom_level"]["text"] != "200%");
    assert!(magnification(&most) <= 8.0 + 1e-9, "held to the limits: {most}");
    run.ok(&["qa", "button", "zoom"]);
    run.ok(&["qa", "button", "zoom_reset"]);
    let fitted = wait_for(&run, |state| (state["app"]["pages"]["magnification"].as_f64().unwrap() - 1.0).abs() < 1e-6);
    assert_eq!(fitted["app"]["zoom_level"]["editing"], false, "{fitted}");
    assert_eq!(run.ok(&["view", "--zoom", "50%"])["zoom_level"], "50%", "the same level an agent can ask for");
    run.ok(&["view", "--sidebar", "off"]);
    let hidden = wait_for(&run, |state| state["app"]["sidebar_visible"] == false);
    assert_eq!((&hidden["app"]["zoom_level"]["visible"], &hidden["app"]["zoom_level"]["in_the_top_right"]), (&json!(true), &json!(true)), "it doesn't go with the sidebar: {hidden}");
    // With the agent icon hidden, the zoom level takes its place, at the same margin from the edge.
    let margin = hidden["app"]["agent_icon_margins"]["right"].as_f64().unwrap();
    run.ok(&["view", "--agent-icon", "off"]);
    let alone = wait_for(&run, |state| state["app"]["agent_icon"] == false);
    let level = &alone["app"]["zoom_level"];
    assert_eq!((&level["visible"], &level["in_the_top_right"], &level["above_the_scroller"]), (&json!(true), &json!(true), &json!(true)), "{alone}");
    assert!((level["right_margin"].as_f64().unwrap() - margin).abs() < 0.5, "the icon's margin: {alone}");
}

/// Failure: closing the window keeps tiles and a worker, so it's over budget, or the app quits, or the tabs are lost. The red button's path, and back. The agent's socket stays.
#[test]
fn closing_the_window_frees_tiles_and_keeps_the_session() {
    let Some(run) = Run::start() else { return };
    // More tiles with ink in them than go on in one turn, so the rest have to be told about after the window's back. With a page of two lines, what's drawn goes on in a turn anyway, and this passed with tiles stopping for good on any fuller page.
    let line = "words and more words, right across the page, ".repeat(3);
    let lines: Vec<&str> = std::iter::repeat_n(line.as_str(), 70).collect();
    let file = TempPdf::new("full.pdf", &pages(&[Page::new(text_lines(&lines))]));
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let before = run.report();
    let layers = before["app"]["pages"]["tile_layers"].as_u64().unwrap();
    assert!(layers > 8, "this needs more than a turn's worth: {before}");
    run.ok(&["qa", "window", "close"]);
    let closed = run.report();
    assert_eq!(closed["reader"]["tiles"]["attached"], 0, "{closed}");
    assert_eq!(closed["reader"]["tiles"]["kept"], 0, "{closed}");
    assert_eq!(closed["app"]["window_open"], false);
    assert_eq!(run.ok(&["status"])["tabs"], 1, "still there");
    let shot = run.state.root.join("closed.png");
    let (reply, code) = run.state.json(&["qa", "shot", "--out", shot.to_str().unwrap()]);
    assert_eq!(code, exit::FAILED, "{reply}");
    run.ok(&["qa", "window", "open"]);
    run.settled();
    let back = run.report();
    assert_eq!(back["app"]["window_open"], true);
    assert_eq!(back["app"]["pages"]["tile_layers"].as_u64().unwrap(), layers, "every tile is back on a layer: {back}");
}

/// Failure: `qa shot` is at Read-only, and an agent can start a QA run itself, so it could replace any file with a PNG, a thesis included. It writes a new `.png` and nothing else: not over a file, not through a link.
#[test]
fn a_shot_replaces_nothing() {
    let Some(run) = Run::start() else { return };
    let root = &run.state.root;
    let refused = |out: &std::path::Path, code: &str| {
        let (reply, status) = run.state.json(&["qa", "shot", "--out", out.to_str().unwrap()]);
        assert_ne!(status, 0, "{reply}");
        assert_eq!(reply["error"]["code"], code, "{reply}");
    };
    let thesis = root.join("thesis.pdf");
    std::fs::write(&thesis, b"years of work").unwrap();
    refused(&thesis, "usage");
    assert_eq!(std::fs::read(&thesis).unwrap(), b"years of work");
    let picture = root.join("picture.png");
    std::fs::write(&picture, b"an earlier one").unwrap();
    refused(&picture, "output_exists");
    assert_eq!(std::fs::read(&picture).unwrap(), b"an earlier one");
    let link = root.join("link.png");
    let aimed_at = root.join("aimed-at.txt");
    std::fs::write(&aimed_at, b"untouched").unwrap();
    std::os::unix::fs::symlink(&aimed_at, &link).unwrap();
    refused(&link, "output_exists");
    assert_eq!(std::fs::read(&aimed_at).unwrap(), b"untouched");
    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    let fine = root.join("new.png");
    let written = run.ok(&["qa", "shot", "--out", fine.to_str().unwrap()]);
    assert_eq!(written["out"], fine.to_str().unwrap(), "{written}");
    assert!(std::fs::read(&fine).unwrap().starts_with(b"\x89PNG"));
    let left: Vec<_> = std::fs::read_dir(root).unwrap().filter_map(Result::ok).filter(|entry| entry.file_name().to_string_lossy().contains("recto-shot")).collect();
    assert!(left.is_empty(), "no temp file left next to it: {left:?}");
}

/// Failure: `qa appearance zzz` reports success, with the rule in the host. It's refused before the host hears of it.
#[test]
fn a_drive_verb_refuses_what_it_cant_do() {
    let Some(run) = Run::start() else { return };
    let (reply, status) = run.state.json(&["qa", "appearance", "zzz"]);
    assert_eq!(status, exit::USAGE, "{reply}");
    assert_eq!(reply["error"]["code"], "usage", "{reply}");
}

/// A PDF of as many pages, each with an outline entry too long for the sidebar.
fn long_titled(count: usize) -> TempPdf {
    let first_page = 4 + count;
    let mut objects = vec![
        Object::Dict("<< /Type /Catalog /Pages 2 0 R /Outlines 3 0 R >>".into()),
        Object::Dict(format!("<< /Type /Pages /Kids [{}] /Count {count} >>", (0..count).map(|index| format!("{} 0 R", first_page + index)).collect::<Vec<_>>().join(" "))),
        Object::Dict(format!("<< /Type /Outlines /First 4 0 R /Last {} 0 R /Count {count} >>", 4 + count - 1)),
    ];
    for index in 0..count {
        let prev = if index > 0 { format!("/Prev {} 0 R", 4 + index - 1) } else { String::new() };
        let next = if index + 1 < count { format!("/Next {} 0 R", 4 + index + 1) } else { String::new() };
        objects.push(Object::Dict(format!("<< /Title (Section {index} with a name that goes on far past the edge of any sidebar there is) /Parent 3 0 R {prev} {next} /Dest [{} 0 R /Fit] >>", first_page + index)));
    }
    for _ in 0..count {
        objects.push(Object::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".into()));
    }
    TempPdf::new("long-titles.pdf", &build(&objects))
}

/// Failure: every other QA verb calls the method behind an event, so a method AppKit can't reach by name passes them all: hover never fired on a real window, as its handlers weren't Objective-C, so full names and hover details did nothing. Events are made and handed to the real responder by AppKit's selector, and what's checked is what comes of them: the pointer resting on a shortened contents entry opens its full name, on the real timer, and goes when it leaves. Nothing is posted.
#[test]
fn what_appkit_calls_reaches_the_app() {
    let Some(run) = Run::start() else { return };
    let long = long_titled(200);
    run.ok(&["open", long.0.to_str().unwrap()]);
    run.settled();
    // Whatever this Mac's mouse is, what's hovering's own rule is the same.
    run.ok(&["qa", "scroller-style", "overlay"]);
    let found = run.ok(&["qa", "entry-points"]);
    let checked = found.as_object().unwrap();
    let failed: Vec<&String> = checked.iter().filter(|(_, ok)| **ok != true).map(|(name, _)| name).collect();
    assert!(failed.is_empty(), "these don't reach the app: {failed:?}\n{found}");
    for name in ["command_plus_zooms_in", "command_minus_zooms_out", "command_9_fits", "command_0_is_actual_size", "plain_plus_zooms_in", "plain_minus_zooms_out", "command_scroll_zooms", "command_scroll_keeps_the_point_still", "more_button", "panel_opens", "panel_button_answers", "panel_closes", "sidebar_area_is_on_the_sidebar", "contents_area_is_on_the_contents", "sidebar_button", "zoom_level", "zoom_field", "zoom_reset", "undo_reaches_the_app_with_the_window_key", "redo_reaches_the_app_with_the_window_key", "hover_shows_details", "pointer_finds_a_tab_row", "contents_hover_shows_their_scroller", "pointer_finds_the_contents_row", "contents_hover_reveals_the_full_name", "contents_hover_ends", "hover_ends", "reveal_goes_with_the_pointer", "plain_wheel_goes_to_the_scroll_view", "wheel_in_contents_stops_following", "pinch_starts", "pinch_ends", "mouse_reaches_the_selection", "pointer_reaches_the_cursor", "pointer_area_is_on_the_page", "right_click_has_a_menu", "copy_is_grey_with_nothing_selected", "select_all_can_be_chosen", "select_all_key_reaches_the_app", "select_all_selects", "copy_is_enabled_with_something_selected", "copy_key_reaches_the_app", "services_arent_offered_what_they_didnt_ask_for", "services_arent_offered_nothing", "services_write_nothing_with_nothing_selected", "tab_row_click", "contents_row_click", "add_button", "home_button"] {
        assert!(checked.contains_key(name), "{name} wasn't checked: {found}");
    }
}

/// Failure: with a mouse connected, or "always show" set, a Mac's scrollers take their width from the view, and a page fitted to the view's whole width is wider than what's left, so a scroll bar sits under it for good and the vertical reading space goes down by it. Seen in a shot, when one was plugged in. A fitted page fits, by either style.
#[test]
fn a_fitted_page_fits_with_classic_scrollers() {
    let Some(run) = Run::start() else { return };
    let file = long_document(5);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    // With no one asking: thin ones that fade, as in Pages, unless this Mac is set to always show them. On automatic with a mouse paired, AppKit puts classic ones back by itself, as the owner saw on a real window.
    let setting = Command::new("/usr/bin/defaults").args(["read", "-g", "AppleShowScrollBars"]).output().unwrap();
    let always = String::from_utf8_lossy(&setting.stdout).trim() == "Always";
    assert_eq!(run.report()["app"]["pages"]["scroller_style"], if always { "legacy" } else { "overlay" });
    for style in ["overlay", "legacy", "overlay"] {
        run.ok(&["qa", "scroller-style", style]);
        run.settled();
        let state = run.report();
        let pages = &state["app"]["pages"];
        assert_eq!(pages["scroller_style"], style, "{pages}");
        assert!(pages["system_scroller_style"].is_string(), "what this Mac is set to is said: {pages}");
        assert!(pages["document_width"].as_f64().unwrap() <= pages["view_width"].as_f64().unwrap() + 0.5, "{style}: {pages}");
        assert_eq!(pages["horizontal_scroller"], false, "{style}: {pages}");
    }
    // Scrollers showing or not don't move the page: with classic ones, a page of a long document and of a one-page one, which has none to show, are the same width.
    let short = long_document(1);
    run.ok(&["qa", "scroller-style", "legacy"]);
    run.settled();
    let long_fit = run.report()["reader"]["fit"].as_f64().unwrap();
    run.ok(&["open", short.0.to_str().unwrap()]);
    run.settled();
    let short_fit = run.report()["reader"]["fit"].as_f64().unwrap();
    assert!((long_fit - short_fit).abs() < 1e-6, "{long_fit} against {short_fit}");
}

/// Failure: closing the window and opening it again sends the reader to page 1 at no zoom, as the new window's first word is taken for them moving. Other tests have a one-page file, which can't see it.
#[test]
fn reopening_the_window_keeps_the_place_and_the_zoom() {
    let Some(run) = Run::start() else { return };
    let file = long_document(60);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "40"]);
    run.settled();
    run.ok(&["qa", "zoom", "2"]);
    run.settled();
    let before = run.report();
    let page = before["reader"]["page"].clone();
    run.ok(&["qa", "window", "close"]);
    // Closed, it's settled: there's nothing to wait for, and nothing held for a window that isn't there.
    run.settled();
    run.ok(&["qa", "window", "open"]);
    run.settled();
    let back = run.report();
    assert_eq!(back["reader"]["page"], page, "{back}");
    assert_eq!(back["app"]["pages"]["magnification"], 2.0, "{back}");
    assert_eq!(run.ok(&["where"])["page"], page, "the session's place too");
}

/// Failure: a tab's file doesn't answer, on a dead volume, and a click on the tab holds the session thread for as long as it takes: later clicks and opens wait, and with the session's lock held so does `status`. A FIFO with no writer stands in for a dead volume: its open never returns. Everything else goes on, on the app's real path.
#[test]
fn a_file_that_doesnt_answer_holds_up_nothing_else() {
    let Some(run) = Run::start() else { return };
    let folder = run.state.root.join("files");
    std::fs::create_dir_all(&folder).unwrap();
    let body = pages(&[Page::new(text_lines(&["A page"]))]);
    let paths: Vec<PathBuf> = (0..5).map(|index| folder.join(format!("{index}.pdf"))).collect();
    for path in &paths {
        std::fs::write(path, &body).unwrap();
        run.ok(&["open", path.to_str().unwrap()]);
    }
    run.settled();
    // The first has gone cold by now. Its file stops answering.
    std::fs::remove_file(&paths[0]).unwrap();
    assert!(Command::new("/usr/bin/mkfifo").arg(&paths[0]).status().unwrap().success());
    let started = std::time::Instant::now();
    run.ok(&["qa", "click-row", "0"]);
    assert_eq!(run.ok(&["status"])["tabs"], 5, "the session answers");
    assert_eq!(run.ok(&["tabs"])["count"].as_u64().or(Some(5)), Some(5));
    // A later click goes ahead, and it's what's shown when the read does come back.
    run.ok(&["qa", "click-row", "2"]);
    let elsewhere = wait_for(&run, |state| state["reader"]["tab"] == 3);
    assert_eq!(elsewhere["reader"]["tab"], 3, "{elsewhere}");
    assert!(started.elapsed() < std::time::Duration::from_secs(15), "{:?}", started.elapsed());
    run.settled();
    let last = folder.join("late.pdf");
    std::fs::write(&last, &body).unwrap();
    run.ok(&["open", last.to_str().unwrap()]);
    run.settled();
    // Lets its thread go, and nothing comes of it: the reader has moved on.
    drop(std::fs::OpenOptions::new().write(true).open(&paths[0]));
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_ne!(run.report()["reader"]["tab"], 1, "a read coming back doesn't take the reader anywhere");
}

/// Failure: the owner couldn't see the settings: the panel is there only if it can be opened, says what's chosen, and changes what it says it does, through its real buttons, with no re-render for warm paper. And at rest there's only the button.
#[test]
fn the_panel_opens_and_does_what_it_says() {
    let Some(run) = Run::start() else { return };
    // In a light window, where the paper with nothing chosen is white, and warm is that with a tint.
    run.ok(&["qa", "appearance", "light"]);
    let file = long_document(5);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let rest = run.report();
    assert_eq!(rest["app"]["panel"]["open"], false, "{rest}");
    assert_eq!(rest["app"]["panel"]["items"].as_array().unwrap().len(), 0, "quiet at rest: {rest}");
    let list_at_rest = rest["app"]["contents"]["list_height"].as_f64().unwrap();
    run.ok(&["qa", "panel", "toggle"]);
    wait_for(&run, |state| state["app"]["panel"]["open"] == true);
    run.settled();
    let open = run.report();
    let ids: Vec<&str> = open["app"]["panel"]["items"].as_array().unwrap().iter().map(|item| item["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["rotate.left", "rotate.right", "paper.white", "paper.warm", "paper.dark"], "this and nothing else: tools first, and zoom is the top left's, by the owner's word");
    assert!(open["app"]["contents"]["list_height"].as_f64().unwrap() < list_at_rest, "it docks in room from the list: {open}");
    let drawn = open["reader"]["counts"]["drawn"].clone();
    run.ok(&["qa", "panel", "paper.warm"]);
    let warm = wait_for(&run, |state| state["app"]["paper"] == "warm");
    assert_eq!(warm["app"]["pages"]["tint_layers"].as_u64().map(|layers| layers >= 1), Some(true), "{warm}");
    assert_eq!(warm["reader"]["counts"]["drawn"], drawn, "warm paper draws nothing");
    let picked: Vec<&Value> = warm["app"]["panel"]["items"].as_array().unwrap().iter().filter(|item| item["selected"] == true).collect();
    assert_eq!(picked.len(), 1);
    assert_eq!(picked[0]["id"], "paper.warm");
    assert_eq!(run.ok(&["view"])["paper"], "warm", "the same setting `recto view` has");
    run.ok(&["qa", "panel", "toggle"]);
    wait_for(&run, |state| state["app"]["panel"]["open"] == false);
}

/// Failure: the menu bar isn't what Rust lists, or the star's margins aren't the same to the right and above, or Show Agent Icon doesn't hide the star, or isn't remembered, or the page's scroller still runs under where the star is, or the full way with a star there. Owner, 2026-11-07: "To save horizontal space the scrollbar can stop below the agent icon. Add an option in the macos top bar to turn off agent bar (hides the icon), enable + - and command - scroll for zooming".
#[test]
fn the_menu_bar_and_the_agent_icon() {
    let Some(run) = Run::start() else { return };
    let file = long_document(5);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let state = run.report();
    let titles: Vec<&str> = state["app"]["menu_bar"].as_array().unwrap().iter().map(|menu| menu["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["RectoPDF", "File", "Edit", "View"], "{state}");
    let view = &state["app"]["menu_bar"][3]["items"];
    let names: Vec<&str> = view.as_array().unwrap().iter().map(|item| item["title"].as_str().unwrap()).collect();
    assert_eq!(names, ["Zoom In", "Zoom Out", "Actual Size", "Fit", "Hide Sidebar", "Single Page", "Side by Side", "Side by Side, First Page Alone", "Show Agent Icon"]);
    assert_eq!(state["app"]["agent_icon"], true);
    // Owner, 2026-11-07: the same margin to the right as above, and less above than the 16 it had.
    let (top, right) = (state["app"]["agent_icon_margins"]["top"].as_f64().unwrap(), state["app"]["agent_icon_margins"]["right"].as_f64().unwrap());
    assert!((top - right).abs() < 0.5 && (8.0..16.0).contains(&top), "top {top}, right {right}");
    assert!(state["app"]["pages"]["scroller_top_inset"].as_f64().unwrap() > 28.0, "below the star: {state}");
    run.ok(&["qa", "menu", "view.agent_icon"]);
    let hidden = wait_for(&run, |state| state["app"]["agent_icon"] == false);
    assert!(hidden["app"]["pages"]["scroller_top_inset"].as_f64().unwrap() < state["app"]["pages"]["scroller_top_inset"].as_f64().unwrap() + 1.0, "no lower than with the star, as the zoom level takes its place: {hidden}");
    assert_eq!(hidden["app"]["menu_bar"][3]["items"][8]["checked"], false, "{hidden}");
    assert_eq!(run.ok(&["view"])["agent_icon"], false, "the same setting `recto view` has");
    assert_eq!(hidden["session"]["view"]["agent_icon_hidden"], true, "remembered, as the state record has it");
    run.ok(&["view", "--agent-icon", "on"]);
    let shown = wait_for(&run, |state| state["app"]["agent_icon"] == true);
    assert_eq!(shown["session"]["view"]["agent_icon_hidden"], false);
    let (reply, status) = run.state.json(&["qa", "menu", "file.open"]);
    assert_eq!(status, exit::USAGE, "a quiet run shows no window: {reply}");
    assert!(run.ok(&["view", "--zoom", "in"])["zoom"].as_f64().unwrap() > 1.0);
}

/// Failure: a file that can't be drawn hangs the view, kills the app, or shows a blank page with no reason. A page past every memory limit must come out as words, with the UI thread free.
#[test]
fn a_page_that_cant_be_drawn_says_so() {
    let Some(run) = Run::start_with(&["--memory-limit-mib", "64"]) else { return };
    let file = TempPdf::new("bomb.pdf", &form_bomb());
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let state = wait_for(&run, |state| state["app"]["note"].is_string());
    let note = state["app"]["note"].as_str().unwrap();
    assert!(note.contains("drawn") || note.contains("memory"), "{note}");
    assert_eq!(state["hangs"]["over_2s"], 0);
}

/// Failure: a large document costs the UI thread per page, or the budget of 2 s is passed just by having one open. A page count no reader will see the end of opens and scrolls like any other.
#[test]
fn a_long_document_is_quick_to_get_around() {
    let Some(run) = Run::start() else { return };
    let file = long_document(5000);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "4800"]);
    run.settled();
    let state = run.report();
    assert!(state["reader"]["visible_pages"].as_array().unwrap().contains(&json!(4800)), "{state}");
    assert!(state["reader"]["tiles"]["page_layers"].as_u64().unwrap() <= 3, "{state}");
    assert_eq!(state["hangs"]["over_2s"], 0);
}

/// Failure: the script runner passes a script whose expectation doesn't hold, or the checked-in script no longer matches what the app does.
#[test]
fn the_reader_script_passes_and_a_wrong_one_fails() {
    let Some(run) = Run::start() else { return };
    let file = long_document(12);
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app/qa/reader.qa");
    let output = run.state.command().args(["qa", "run", script.to_str().unwrap(), "--json"]).env("RECTO_QA_PDF", &file.0).output().unwrap();
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["data"]["passed"], true);
    let wrong = run.state.root.join("wrong.qa");
    std::fs::write(&wrong, "qa state\nexpect app.is_active true\n").unwrap();
    let (reply, code) = run.state.json(&["qa", "run", wrong.to_str().unwrap()]);
    assert_eq!(code, exit::FAILED, "{reply}");
    assert!(reply["error"]["message"].as_str().unwrap().contains("line 2"), "{reply}");
}

/// Failure: updating the sidebar leaves something behind each time, so memory grows just from reading. On macOS 27 an `NSTextField` left about 160 bytes in AppKit at each layout, for good: 7 KiB for each tab opened, found at 5.7 MiB over the budget of 4 at 1,000 tabs. Across hundreds of page crossings with a sidebar full of rows, what's kept must stay flat, and only what's kept is counted, since tiles are discardable.
#[test]
fn reading_doesnt_grow_the_app() {
    let Some(run) = Run::start() else { return };
    let titles: Vec<String> = (0..24).map(|index| format!("<< /Title (Section {index}) /Parent 3 0 R {} {} /Dest [{} 0 R /Fit] >>", if index > 0 { format!("/Prev {} 0 R", 4 + index - 1) } else { String::new() }, if index < 23 { format!("/Next {} 0 R", 4 + index + 1) } else { String::new() }, 28)).collect();
    let mut objects = vec![Object::Dict("<< /Type /Catalog /Pages 2 0 R /Outlines 3 0 R >>".into()), Object::Dict("<< /Type /Pages /Kids [28 0 R 29 0 R] /Count 2 >>".into()), Object::Dict("<< /Type /Outlines /First 4 0 R /Last 27 0 R /Count 24 >>".into())];
    objects.extend(titles.into_iter().map(Object::Dict));
    for _ in 0..2 {
        objects.push(Object::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".into()));
    }
    let file = TempPdf::new("rows.pdf", &build(&objects));
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let rows = run.report()["app"]["sidebar_rows"].as_u64().unwrap();
    assert!(rows >= 20, "{rows}");
    // What's allocated, and the footprint less what's in the compressor. The footprint alone can't be held to a megabyte: under the whole Mac's memory pressure the system compresses pages of system libraries' data the app shares and never touched again, and the footprint charges them once they're compressed. That's what failed this test with 2.4 to 2.9 MB, on this branch, while the update tests beside it raised the pressure: by region, the heap was unchanged and the rise was those libraries' `__DATA`, `__AUTH` and CoreUI pages going to the compressor. On main it was seen falling by as much. A real leak shows in the heap, as AppKit's did, or in what's resident.
    let measure = || {
        let memory = run.report()["memory"].clone();
        let footprint = memory["footprint_bytes"].as_u64().unwrap();
        (memory["heap_bytes"].as_u64().unwrap(), footprint.saturating_sub(memory["compressed_bytes"].as_u64().unwrap()), footprint)
    };
    // The first few crossings make what's reused from then on.
    for page in ["2", "1", "2", "1"] {
        run.ok(&["goto", page]);
        run.settled();
    }
    let (heap, resident, footprint) = measure();
    for round in 0..400 {
        run.ok(&["goto", if round % 2 == 0 { "2" } else { "1" }]);
    }
    run.settled();
    let (heap_after, resident_after, footprint_after) = measure();
    let said = format!("over 400 page crossings: heap {heap} to {heap_after}, footprint less compressed {resident} to {resident_after}, footprint {footprint} to {footprint_after}");
    assert!(heap_after.saturating_sub(heap) < 1024 * 1024, "the heap grew, {said}");
    assert!(resident_after.saturating_sub(resident) < 1024 * 1024, "the app grew, {said}");
}

/// Failure: a long outline pushes the other tabs and the footer off the sidebar, the reason the owner asked for this, at a normal height or a short one. Or the rows of a long outline are all built. Or reading drags the contents back from where someone has scrolled them, or they lose the place while reading on.
#[test]
fn a_long_outline_scrolls_by_itself_and_keeps_the_tabs_on_screen() {
    let Some(run) = Run::start() else { return };
    run.ok(&["qa", "scroller-style", "overlay"]);
    let long = outlined_with(2000);
    let others: Vec<TempPdf> = (0..4).map(|_| long_document(1)).collect();
    run.ok(&["open", long.0.to_str().unwrap()]);
    for other in &others {
        run.ok(&["open", other.0.to_str().unwrap()]);
    }
    run.ok(&["tap", "#1"]);
    run.settled();
    let state = run.report();
    let contents = &state["app"]["contents"];
    assert_eq!(contents["count"], 2000, "{contents}");
    assert!(contents["height"].as_f64().unwrap() > 0.0, "{contents}");
    assert_eq!(contents["tabs_scroll"], false, "five tabs fit in what's left: {contents}");
    assert!(contents["height"].as_f64().unwrap() + 5.0 * 32.0 <= contents["list_height"].as_f64().unwrap() + 0.5, "{contents}");
    assert_eq!(contents["tab_rows_on_screen"], 5, "all the tabs show: {contents}");
    assert_eq!(contents["footer_on_screen"], true);
    assert!(contents["height"].as_f64().unwrap() <= contents["list_height"].as_f64().unwrap() * 0.6 + 0.5, "it stops at the maximum: {contents}");
    assert!(contents["row_views"].as_u64().unwrap() < 80, "views for what's on screen, not 2,000: {contents}");
    // A short window.
    let short = run.ok(&["qa", "resize", "900x420"]);
    assert!(short["contents"]["height"].as_f64().unwrap() <= short["contents"]["list_height"].as_f64().unwrap() * 0.6 + 0.5, "the same maximum, and the tabs scroll in what's left: {short}");
    assert_eq!(short["contents"]["footer_on_screen"], true, "{short}");
    assert_eq!(short["contents"]["current_in_view"], true, "a shorter window doesn't lose the place: {short}");
    run.ok(&["qa", "resize", "1440x900"]);
    run.settled();
    // The place stays in view as the reader reads on, and a far `goto` lands rather than whooshes.
    run.ok(&["goto", "1500"]);
    run.settled();
    let reading = wait_for(&run, |state| state["app"]["contents"]["current_in_view"] == true && state["app"]["contents"]["frames_running"] == false && state["app"]["contents"]["current"].as_u64().unwrap_or(0) > 1000);
    assert_eq!(reading["app"]["contents"]["following"], true);
    assert_eq!(reading["app"]["contents"]["moving"], false, "nothing runs once it's arrived: {reading}");
    // Failure: rows the table had ready just outside the view keep an old rail, and show it when they scroll in. Seen on a far `goto` back to the top.
    run.ok(&["goto", "1"]);
    run.settled();
    let back = wait_for(&run, |state| state["app"]["contents"]["frames_running"] == false && state["app"]["contents"]["current"].as_u64().unwrap_or(9) == 0);
    assert_eq!(back["app"]["contents"]["stale_rows"], 0, "{back}");
    run.ok(&["goto", "1500"]);
    run.settled();
    let again = wait_for(&run, |state| state["app"]["contents"]["frames_running"] == false && state["app"]["contents"]["current"].as_u64().unwrap_or(0) > 1000);
    assert_eq!(again["app"]["contents"]["stale_rows"], 0, "{again}");
    // Failure: every reposition flashes a scroller the reader didn't ask for, where the sidebar is meant to be names and contents at rest.
    assert_eq!(again["app"]["contents"]["scroller"], false, "{again}");
    // Scrolled away, it stays where it's put, however the reader moves on.
    let away = run.ok(&["qa", "contents-scroll", "10"]);
    assert_eq!(away["contents"]["following"], false, "{away}");
    let scrolled_to = away["contents"]["scroll_y"].as_f64().unwrap();
    run.ok(&["goto", "1700"]);
    run.settled();
    let still = wait_for(&run, |state| state["app"]["contents"]["current"].as_u64().unwrap_or(0) > 1600);
    assert_eq!(still["app"]["contents"]["scroll_y"].as_f64().unwrap(), scrolled_to, "{still}");
    assert_eq!(still["app"]["contents"]["current_in_view"], false, "{still}");
    assert_eq!(still["app"]["contents"]["frames_running"], false);
    // With the pointer out and a few seconds gone, the next change of section picks it up. Time is the scenario here.
    run.ok(&["qa", "contents-pointer", "out"]);
    std::thread::sleep(std::time::Duration::from_millis(3100));
    run.ok(&["goto", "1800"]);
    run.settled();
    wait_for(&run, |state| state["app"]["contents"]["following"] == true && state["app"]["contents"]["current_in_view"] == true && state["app"]["contents"]["frames_running"] == false);
}

/// Where the app's threads are over the next 2 s, for a test that finds it awake when it should be resting. CI's machines don't behave like this one, so a failure there has to carry its own evidence.
fn whats_awake(pid: u32) -> String {
    let file = std::env::temp_dir().join(format!("recto-awake-{pid}.txt"));
    let sampled = std::process::Command::new("/usr/bin/sample").arg(pid.to_string()).args(["2", "-mayDie", "-file"]).arg(&file).output();
    let text = std::fs::read_to_string(&file).unwrap_or_else(|_| format!("no sample: {sampled:?}"));
    let _ = std::fs::remove_file(&file);
    // The stacks that were running most, then the threads. Long lines and the library list say nothing here.
    let from = text.find("Call graph:").unwrap_or(0);
    let to = text.find("Binary Images:").unwrap_or(text.len());
    text[from..to].lines().take(320).map(|line| format!("{}\n", line.chars().take(130).collect::<String>())).collect()
}

/// Failure: following makes page scrolling slower, the contents stutter, the place drops out of view while reading, or something's left running once they've arrived. Records the contents on every step of a real sweep, and holds the properties: a small largest step, the place in view, and no frames or wake-ups at rest.
#[test]
fn the_contents_follow_quietly_and_nothing_runs_at_rest() {
    let Some(run) = Run::start() else { return };
    let long = outlined_with(600);
    run.ok(&["open", long.0.to_str().unwrap()]);
    run.settled();
    // Past the first screenful, or the place stays in the dead zone at the top and nothing need move. And the move that goes there ends first.
    run.ok(&["goto", "100"]);
    run.settled();
    wait_for(&run, |state| state["app"]["contents"]["frames_running"] == false);
    let sweep = run.ok(&["qa", "sweep", "6", "--for", "4s"]);
    let contents = &sweep["contents"];
    let steps = sweep["steps"].as_u64().unwrap() as f64;
    assert!(steps >= 1.0, "{sweep}");
    assert!(contents["moves"].as_u64().unwrap() >= 1, "they did follow: {sweep}");
    // Reposition, then rest: 6 sections a second past a dead zone of about 12 rows is a move every 2 s or so. Well below one a section.
    assert!(contents["moves"].as_u64().unwrap() <= 6, "{sweep}");
    assert!(contents["rest_s"]["median"].as_f64().unwrap() >= 0.4, "{sweep}");
    assert!(contents["steps_with_place_in_view"].as_f64().unwrap() >= steps * 0.95, "{sweep}");
    let settled = wait_for(&run, |state| state["app"]["contents"]["frames_running"] == false);
    assert_eq!(settled["app"]["contents"]["moving"], false);
    run.settled();
    // With nothing to do there's nothing to wake for. Something left running never gets to the first quiet window, and something that wakes now and then fails the windows after it, so neither passes for rest.
    // The wait is for a burst seen only on CI's virtual machines, about 80 wake-ups a second for up to 6 s after settling, while a sample of every thread found none of them working. This Mac reads 0 to 4 throughout. The CPU it costs is reported, and `docs/PLAN.md` tracks finding out what it is.
    let pid = run.pid;
    let sampling = std::thread::spawn(move || whats_awake(pid));
    let window = || {
        let before = recto_platform::local::process_counters(pid).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(2));
        let after = recto_platform::local::process_counters(pid).unwrap();
        (after.wakeups.saturating_sub(before.wakeups), after.cpu_ns.saturating_sub(before.cpu_ns) / 1_000_000)
    };
    let mut settling = Vec::new();
    while settling.len() < 8 {
        settling.push(window());
        if settling.last().unwrap().0 <= 8 {
            break;
        }
    }
    let resting: Vec<(u64, u64)> = (0..3).map(|_| window()).collect();
    eprintln!("wake-ups and CPU ms in each 2 s: settling {settling:?}, at rest {resting:?}");
    let evidence = format!("settling {settling:?}, at rest {resting:?}\nstate: {}\n{}", run.report(), sampling.join().unwrap());
    assert!(settling.last().unwrap().0 <= 8, "it never came to rest: {evidence}");
    assert!(resting.iter().all(|(woke, _)| *woke <= 80), "it wakes at rest: {evidence}");
    // Reduce Motion: it stays in view, with no frames at all.
    run.ok(&["qa", "reduce-motion", "on"]);
    run.ok(&["goto", "400"]);
    run.settled();
    let quiet = wait_for(&run, |state| state["app"]["contents"]["current"].as_u64().unwrap_or(0) > 300);
    assert_eq!(quiet["app"]["contents"]["frames_running"], false, "{quiet}");
    assert_eq!(quiet["app"]["contents"]["current_in_view"], true, "{quiet}");
}

/// A PDF of as many pages, each with an outline entry.
fn outlined_with(count: usize) -> TempPdf {
    let first_page = 4 + count;
    let mut objects = vec![
        Object::Dict("<< /Type /Catalog /Pages 2 0 R /Outlines 3 0 R >>".into()),
        Object::Dict(format!("<< /Type /Pages /Kids [{}] /Count {count} >>", (0..count).map(|index| format!("{} 0 R", first_page + index)).collect::<Vec<_>>().join(" "))),
        Object::Dict(format!("<< /Type /Outlines /First 4 0 R /Last {} 0 R /Count {count} >>", 4 + count - 1)),
    ];
    for index in 0..count {
        let prev = if index > 0 { format!("/Prev {} 0 R", 4 + index - 1) } else { String::new() };
        let next = if index + 1 < count { format!("/Next {} 0 R", 4 + index + 1) } else { String::new() };
        objects.push(Object::Dict(format!("<< /Title (Section {index}) /Parent 3 0 R {prev} {next} /Dest [{} 0 R /Fit] >>", first_page + index)));
    }
    for _ in 0..count {
        objects.push(Object::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".into()));
    }
    TempPdf::new("long-outline.pdf", &build(&objects))
}

/// A PDF with chapters of sections, each on a page, so the contents open and close as the reader moves.
fn chaptered(chapters: usize, sections: usize) -> TempPdf {
    let each = sections + 1;
    let total = chapters * each;
    let first_page = 4 + total;
    let item = |chapter: usize, section: usize| 4 + chapter * each + section;
    let mut objects = vec![
        Object::Dict("<< /Type /Catalog /Pages 2 0 R /Outlines 3 0 R >>".into()),
        Object::Dict(format!("<< /Type /Pages /Kids [{}] /Count {total} >>", (0..total).map(|index| format!("{} 0 R", first_page + index)).collect::<Vec<_>>().join(" "))),
        Object::Dict(format!("<< /Type /Outlines /First 4 0 R /Last {} 0 R /Count {chapters} >>", item(chapters - 1, 0))),
    ];
    for chapter in 0..chapters {
        let prev = if chapter > 0 { format!("/Prev {} 0 R", item(chapter - 1, 0)) } else { String::new() };
        let next = if chapter + 1 < chapters { format!("/Next {} 0 R", item(chapter + 1, 0)) } else { String::new() };
        objects.push(Object::Dict(format!("<< /Title (Chapter {chapter}) /Parent 3 0 R {prev} {next} /First {} 0 R /Last {} 0 R /Count -{sections} /Dest [{} 0 R /Fit] >>", item(chapter, 1), item(chapter, sections), first_page + chapter * each)));
        for section in 1..=sections {
            let prev = if section > 1 { format!("/Prev {} 0 R", item(chapter, section - 1)) } else { String::new() };
            let next = if section < sections { format!("/Next {} 0 R", item(chapter, section + 1)) } else { String::new() };
            objects.push(Object::Dict(format!("<< /Title (Section {chapter}.{section}) /Parent {} 0 R {prev} {next} /Dest [{} 0 R /Fit] >>", item(chapter, 0), first_page + chapter * each + section)));
        }
    }
    for _ in 0..total {
        objects.push(Object::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".into()));
    }
    TempPdf::new("chapters.pdf", &build(&objects))
}

/// Failure: a chapter's change is published once and the next look republishes "nothing changed", and the host reads only the latest, so the table keeps its old row count, and later row changes go at the wrong places. Or a change is applied twice. The table must hold exactly what the model says through every kind of move, and reading forward never moves the contents back.
#[test]
fn the_contents_table_follows_chapters_opening_and_closing() {
    let Some(run) = Run::start() else { return };
    let (chapters, sections) = (40, 8);
    let file = chaptered(chapters, sections);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let holds = |state: &Value, when: &str| {
        let contents = &state["app"]["contents"];
        assert_eq!(contents["table_rows"], contents["count"], "{when}: {contents}");
        assert_eq!(contents["stale_rows"], 0, "{when}: {contents}");
        assert_eq!(contents["current_in_view"], true, "{when}: {contents}");
    };
    let at_rest = |state: &Value| state["app"]["contents"]["frames_running"] == false;
    for chapter in [1, 2, 3, 10, 9, 30, 0] {
        run.ok(&["goto", &(chapter * (sections + 1) + 1).to_string()]);
        run.settled();
        let state = wait_for(&run, at_rest);
        holds(&state, &format!("chapter {chapter}"));
        assert_eq!(state["app"]["contents"]["count"].as_u64().unwrap() as usize, chapters + sections, "one chapter open: {state}");
    }
    let sweep = run.ok(&["qa", "sweep", "6", "--for", "6s"]);
    let back: Vec<&Value> = sweep["contents"]["listed"].as_array().unwrap().iter().filter(|one| one["points"].as_f64().unwrap() < -1.0).collect();
    assert!(back.is_empty(), "reading forward, and the contents moved back: {back:?} of {}", sweep["contents"]);
    run.settled();
    holds(&wait_for(&run, at_rest), "after a sweep");
    run.ok(&["qa", "sweep", "60", "--for", "3s"]);
    run.settled();
    holds(&wait_for(&run, at_rest), "after a fast sweep");
}

/// Failure: closing a warm tab leaves its document, tiles and worker held until the next switch of tab, which may never come.
#[test]
fn closing_a_warm_tab_lets_go_of_it() {
    let Some(run) = Run::start() else { return };
    let (one, two) = (long_document(2), TempPdf::new("second.pdf", &pages(&[Page::new(text_lines(&["Second"]))])));
    run.ok(&["open", one.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["open", two.0.to_str().unwrap()]);
    run.settled();
    assert_eq!(run.report()["reader"]["documents_held"], 2, "the first is warm");
    run.ok(&["close", "--tab", "#1"]);
    let state = wait_for(&run, |state| state["reader"]["documents_held"] == 1);
    assert_eq!(state["reader"]["documents_held"], 1);
}

/// Failure: a shot's tint isn't clipped to where the pages show, so a zoomed page's rectangle tints the sidebar in the picture, though the compositor's own tint never would.
#[test]
fn a_shots_tint_stays_on_the_pages() {
    let Some(run) = Run::start() else { return };
    let file = long_document(2);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["view", "--paper", "warm"]);
    run.ok(&["qa", "zoom", "6"]);
    run.settled();
    let state = run.report();
    assert!(state["app"]["pages"]["shot_tint_left"].as_f64().unwrap() >= 200.0, "the sidebar is left of this: {state}");
}

/// Failure: warm paper redraws every tile, so it's slow and costs memory, or it changes what's saved and exported, or it doesn't work in a dark window, or it isn't remembered. Going to it draws nothing, `recto render` gives the same bytes, every paper works in every window, and a new run on this state has it back.
#[test]
fn warm_paper_only_changes_what_is_on_screen() {
    let Some(run) = Run::start() else { return };
    let file = long_document(2);
    run.ok(&["qa", "appearance", "light"]);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let (before, after) = (run.state.root.join("before.png"), run.state.root.join("after.png"));
    run.ok(&["render", "--page", "1", "--out", before.to_str().unwrap()]);
    let drawn = run.report()["reader"]["counts"]["drawn"].as_u64().unwrap();
    assert_eq!(run.ok(&["view", "--paper", "warm"])["paper"], "warm");
    run.settled();
    let warm = wait_for(&run, |state| state["app"]["paper"] == "warm");
    assert_eq!(warm["reader"]["counts"]["drawn"].as_u64().unwrap(), drawn, "no tile redrawn: {warm}");
    assert_eq!(warm["app"]["pages"]["tint_layers"], warm["app"]["pages"]["page_layers"], "{warm}");
    run.ok(&["render", "--page", "1", "--out", after.to_str().unwrap()]);
    assert_eq!(std::fs::read(&before).unwrap(), std::fs::read(&after).unwrap(), "an export is the true page");
    // Dark paper too, as `render` says it always has been.
    run.ok(&["view", "--paper", "dark"]);
    run.settled();
    let after_dark = run.state.root.join("after-dark.png");
    run.ok(&["render", "--page", "1", "--out", after_dark.to_str().unwrap()]);
    assert_eq!(std::fs::read(&before).unwrap(), std::fs::read(&after_dark).unwrap());
    // All six.
    for look in ["light", "dark"] {
        run.ok(&["qa", "appearance", look]);
        for paper in ["white", "warm", "dark"] {
            run.ok(&["view", "--paper", paper]);
            run.settled();
            let shown = wait_for(&run, |state| state["app"]["paper"] == paper);
            assert_eq!(shown["reader"]["paper"], if paper == "dark" { "dark" } else { "white" }, "{look} {paper}");
            assert_eq!(shown["app"]["pages"]["tint_layers"].as_u64().unwrap() > 0, paper == "warm", "{look} {paper}");
        }
    }
    run.ok(&["view", "--paper", "warm"]);
    run.settled();
    assert_eq!(run.state.json(&["view", "--paper", "sepia"]).1, exit::USAGE);
    let root = run.state.root.clone();
    let app = app_path().unwrap();
    run.ok(&["stop"]);
    for _ in 0..200 {
        if !still_running(run.pid) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let output = Command::new(bin()).env(recto_control::state::STATE_ENV, &root).args(["qa", "start", "--app", app.to_str().unwrap(), "--time-limit", "1m", "--json"]).stdin(std::process::Stdio::null()).output().unwrap();
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    let (view, _) = run.state.json(&["view"]);
    assert_eq!(view["data"]["paper"], "warm", "remembered");
    let _ = run.state.json(&["stop"]);
}

/// Failure: a shortened name has no way to be read in full, or the reveal moves rows or lingers. By the method the pointer resting there calls. It opens past the sidebar's edge if it must, with the whole of the text, and goes when asked. A row that isn't cut off shows nothing.
#[test]
fn a_shortened_name_can_be_read_in_full() {
    let Some(run) = Run::start() else { return };
    let long_name = "a-really-long-file-name-that-will-not-fit-in-the-sidebar-at-all-and-goes-on-and-on-and-on";
    let dir = std::env::temp_dir().canonicalize().unwrap().join(format!("recto-reveal-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{long_name}.pdf"));
    std::fs::write(&path, pages(&[Page::new(text_lines(&["A page"]))])).unwrap();
    let short = long_document(1);
    run.ok(&["open", path.to_str().unwrap()]);
    run.ok(&["open", short.0.to_str().unwrap()]);
    run.settled();
    let before = run.report();
    let shown = run.ok(&["qa", "reveal", "0"]);
    let reveal = &shown["reveal"];
    assert_eq!(reveal["cut_off"], true, "{reveal}");
    assert_eq!(reveal["shown"], true, "{reveal}");
    assert_eq!(reveal["is_file_name"], true);
    assert_eq!(reveal["untrusted"]["text"], long_name);
    assert!(reveal["frame_right"].as_f64().unwrap() > reveal["sidebar_width"].as_f64().unwrap(), "past the sidebar's edge: {reveal}");
    assert_eq!(shown["sidebar_rows"], before["app"]["sidebar_rows"], "nothing moved");
    let plain = run.ok(&["qa", "reveal", "1"]);
    assert_eq!((plain["reveal"]["cut_off"].clone(), plain["reveal"]["shown"].clone()), (json!(false), json!(false)), "{plain}");
    assert_eq!(run.ok(&["qa", "reveal", "none"])["reveal"]["shown"], false);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Two pages with two columns of words, the left drawn first: what selecting across a gutter and a page break is about.
fn columns_and_pages() -> TempPdf {
    let page = |name: &str| {
        let mut content = String::from("BT /F1 12 Tf 14 TL 72 720 Td\n");
        for line in ["left one", "left two", "left three"] {
            content.push_str(&format!("({name} {line}) Tj T*\n"));
        }
        content.push_str("ET\nBT /F1 12 Tf 14 TL 340 720 Td\n");
        for line in ["right one", "right two", "right three"] {
            content.push_str(&format!("({name} {line}) Tj T*\n"));
        }
        content.push_str("ET\n");
        Page::new(content.into_bytes())
    };
    TempPdf::new("columns.pdf", &pages(&[page("First"), page("Second")]))
}

/// Where a rectangle of `recto select` is, as `qa mouse-*` takes it: this far along it, in the middle of its height.
fn place_on(run: &Run, page: u32, rect: usize, along: f64) -> String {
    let found = run.ok(&["select", &page.to_string()]);
    let rect = &found["pages"][0]["rects"][rect];
    format!("{page}:{},{}", rect[0].as_f64().unwrap() + rect[2].as_f64().unwrap() * along, rect[1].as_f64().unwrap() + rect[3].as_f64().unwrap() / 2.0)
}

fn copied(run: &Run) -> String {
    run.ok(&["qa", "copied"])["untrusted"]["text"].as_str().unwrap_or_default().to_string()
}

fn copy_now(run: &Run) {
    run.ok(&["qa", "menu", "edit.copy"]);
    run.settled();
}

fn menu_item(state: &Value, id: &str) -> Value {
    state["app"]["menu_bar"].as_array().unwrap().iter().flat_map(|menu| menu["items"].as_array().unwrap()).find(|item| item["id"] == id).unwrap_or_else(|| panic!("{id} isn't on the menu bar: {state}")).clone()
}

/// Failure: a drag in the real app selects something other than `recto select` says, draws into a page's tiles instead of a layer of its own, or copies in the wrong order. The whole path: window coordinates, the document's, Rust, the layer, Edit > Copy, the clipboard. And the real clipboard isn't the one a quiet run uses.
#[test]
fn a_drag_over_columns_selects_and_copies_in_reading_order_in_the_app() {
    let Some(run) = Run::start() else { return };
    let file = columns_and_pages();
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let tiles_before = run.report()["reader"]["counts"]["drawn"].clone();
    // The second line of the left column, to the second of the right.
    run.ok(&["qa", "mouse-down", &place_on(&run, 1, 1, 0.0)]);
    run.ok(&["qa", "mouse-drag", &place_on(&run, 1, 4, 1.0)]);
    run.ok(&["qa", "mouse-up"]);
    run.settled();
    let state = run.report();
    assert_eq!(state["reader"]["selection"]["selected"], true, "{state}");
    assert_eq!(state["app"]["pages"]["highlight_layers"], 1, "{state}");
    assert_eq!(state["app"]["pages"]["highlight_rects"], 4, "one a line: {state}");
    assert_eq!(state["reader"]["counts"]["drawn"], tiles_before, "selecting draws no tile: {state}");
    copy_now(&run);
    assert_eq!(copied(&run), "First left two\nFirst left three\nFirst right one\nFirst right two");
    assert!(run.ok(&["qa", "copied"])["clipboard"].as_str().unwrap().starts_with("com.rectopdf.qa."), "a quiet run never touches the real clipboard");
    assert_eq!(run.report()["hangs"]["over_2s"], 0);
    // What the Services menu is given is that same text, there and then, since it can't wait.
    let services = run.ok(&["qa", "services"]);
    assert_eq!((&services["offers_text"], &services["written"]), (&json!(true), &json!(true)), "{services}");
    assert_eq!(services["untrusted"]["text"].as_str().unwrap(), copied(&run));
    run.ok(&["qa", "mouse-down", &place_on(&run, 1, 1, 0.0)]);
    run.ok(&["qa", "mouse-up"]);
    run.settled();
    assert_eq!(run.ok(&["qa", "services"])["offers_text"], false, "with nothing selected it offers nothing");
}

/// Failure: a selection stops at a page's edge, or draws only on one page, or Select All means the page in view, when on a Mac it's the document.
#[test]
fn a_selection_crosses_pages_and_select_all_is_the_document() {
    let Some(run) = Run::start() else { return };
    let file = columns_and_pages();
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "zoom", "0.5"]);
    run.settled();
    let visible = run.report()["reader"]["visible_pages"].as_array().unwrap().len();
    assert!(visible >= 2, "both pages have to show for this: {visible}");
    run.ok(&["qa", "mouse-down", &place_on(&run, 1, 5, 0.0)]);
    run.ok(&["qa", "mouse-drag", &place_on(&run, 2, 0, 1.0)]);
    run.ok(&["qa", "mouse-up"]);
    run.settled();
    let state = run.report();
    assert_eq!(state["app"]["pages"]["highlight_layers"], 2, "a layer on each page: {state}");
    copy_now(&run);
    assert_eq!(copied(&run), "First right three\nSecond left one");
    run.ok(&["qa", "menu", "edit.select_all"]);
    run.settled();
    assert_eq!(run.report()["reader"]["selection"]["end"]["page"], 2);
    copy_now(&run);
    let all = copied(&run);
    assert!(all.starts_with("First left one") && all.ends_with("Second right three"), "{all}");
    assert_eq!(all, run.ok(&["copy", "--all"])["untrusted"]["text"], "the same as `recto copy --all`");
}

/// Failure: at another zoom a click lands on another word, since the view's coordinates and the page's differ by the fit and the magnification. It goes in by the window's coordinates, as the mouse does, so what's between them is what's being tested.
#[test]
fn a_click_lands_on_the_same_word_at_any_zoom_in_the_app() {
    let Some(run) = Run::start() else { return };
    let file = TempPdf::new("zoom.pdf", &pages(&[Page::new(text_lines(&["Alpha beta gamma delta epsilon zeta"]))]));
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let found = run.ok(&["select", "--page", "1", "--chars", "11-16"]);
    assert_eq!(found["untrusted"]["text"], "gamma");
    let rect = &found["pages"][0]["rects"][0];
    let place = format!("1:{},{}:2", rect[0].as_f64().unwrap() + rect[2].as_f64().unwrap() / 2.0, rect[1].as_f64().unwrap() + rect[3].as_f64().unwrap() / 2.0);
    for zoom in ["1", "0.5", "1.5", "2.5", "4"] {
        run.ok(&["qa", "zoom", zoom]);
        run.settled();
        run.ok(&["qa", "mouse-down", &place]);
        run.ok(&["qa", "mouse-up"]);
        run.settled();
        copy_now(&run);
        assert_eq!(copied(&run), "gamma", "at {zoom}x");
        let state = run.report();
        assert_eq!(state["app"]["pages"]["highlight_layers"], 1, "at {zoom}x: {state}");
    }
}

/// Failure: a word on a rotated page can't be clicked, because the page's layer and the page's boxes are turned differently in the host and the worker.
#[test]
fn a_click_on_a_rotated_page_in_the_app_takes_what_is_there() {
    let Some(run) = Run::start() else { return };
    let file = TempPdf::new("turned.pdf", &pages(&[Page { content: text_lines(&["Words on a turned page"]), page_extra: "/Rotate 90".to_string(), media: (612.0, 792.0) }]));
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let found = run.ok(&["select", "--page", "1", "--chars", "0-5"]);
    let rect = &found["pages"][0]["rects"][0];
    run.ok(&["qa", "mouse-down", &format!("1:{},{}:2", rect[0].as_f64().unwrap() + rect[2].as_f64().unwrap() / 2.0, rect[1].as_f64().unwrap() + rect[3].as_f64().unwrap() / 2.0)]);
    run.ok(&["qa", "mouse-up"]);
    run.settled();
    copy_now(&run);
    assert_eq!(copied(&run), "Words");
}

/// Failure: Copy is enabled with nothing selected or stays grey with something, Select All works with nothing open, a shift-click doesn't extend, or a click elsewhere leaves a highlight up. What AppKit says of the real menu items, and not what Rust's rule would.
#[test]
fn the_menu_goes_with_the_selection_and_clicks_do_what_they_do_on_a_mac() {
    let Some(run) = Run::start() else { return };
    let home = run.report();
    assert_eq!(menu_item(&home, "edit.copy")["enabled"], false, "{home}");
    assert_eq!(menu_item(&home, "edit.select_all")["enabled"], false, "nothing's open: {home}");
    let file = TempPdf::new("words.pdf", &pages(&[Page::new(text_lines(&["Alpha beta gamma delta", "Second line of words"]))]));
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let open = run.report();
    assert_eq!(menu_item(&open, "edit.copy")["enabled"], false, "nothing selected: {open}");
    assert_eq!(menu_item(&open, "edit.select_all")["enabled"], true, "{open}");
    assert_eq!(menu_item(&open, "edit.copy")["key"], "c");
    let first = place_on(&run, 1, 0, 0.5);
    run.ok(&["qa", "mouse-down", &format!("{first}:3")]);
    run.ok(&["qa", "mouse-up"]);
    run.settled();
    let line = run.report();
    assert_eq!(line["reader"]["selection"]["unit"], "line", "a third click takes the line: {line}");
    assert_eq!(menu_item(&line, "edit.copy")["enabled"], true, "{line}");
    copy_now(&run);
    assert_eq!(copied(&run), "Alpha beta gamma delta");
    // A click puts the place there, and shift extends from it.
    run.ok(&["qa", "mouse-down", &place_on(&run, 1, 0, 0.0)]);
    run.ok(&["qa", "mouse-up"]);
    run.settled();
    let cleared = run.report();
    assert_eq!(cleared["reader"]["selection"]["selected"], false, "{cleared}");
    assert_eq!(cleared["app"]["pages"]["highlight_layers"], 0, "{cleared}");
    assert_eq!(menu_item(&cleared, "edit.copy")["enabled"], false, "{cleared}");
    run.ok(&["qa", "mouse-down", &format!("{}:1:shift", place_on(&run, 1, 1, 1.0))]);
    run.ok(&["qa", "mouse-up"]);
    run.settled();
    copy_now(&run);
    assert_eq!(copied(&run), "Alpha beta gamma delta\nSecond line of words");
}

/// Failure: the I-beam shows over blank paper or the margin, or never comes, or a page with no text gets one.
#[test]
fn the_pointer_is_an_i_beam_over_text_and_only_there() {
    let Some(run) = Run::start() else { return };
    let scan = Page::new(b"0.5 g 100 100 300 300 re f".to_vec());
    let file = TempPdf::new("pointer.pdf", &pages(&[Page::new(text_lines(&["Words on the first page"])), scan]));
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "zoom", "0.5"]);
    run.settled();
    let on_text = place_on(&run, 1, 0, 0.5);
    let cursor = |place: &str| run.ok(&["qa", "pointer", place]);
    cursor(&on_text);
    // Its page is read when there's nothing better to do. A move after that says what's there.
    run.settled();
    assert_eq!(cursor(&on_text)["pages"]["cursor"], "ibeam");
    assert_eq!(cursor("1:300,600")["pages"]["cursor"], "arrow", "blank paper");
    assert_eq!(cursor("1:-20,68")["pages"]["cursor"], "arrow", "the margin");
    cursor("2:200,300");
    run.settled();
    assert_eq!(cursor("2:200,300")["pages"]["cursor"], "arrow", "a page with no text");
}

/// Failure: a scan errors or selects something when dragged over, Copy is offered with nothing to copy, or the UI thread waits on it.
#[test]
fn a_page_with_no_text_in_the_app_selects_nothing() {
    let Some(run) = Run::start() else { return };
    let file = TempPdf::new("scan.pdf", &pages(&[Page::new(b"0.5 g 100 100 300 300 re f".to_vec())]));
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "mouse-down", "1:150,500:2"]);
    run.ok(&["qa", "mouse-drag", "1:300,600"]);
    run.ok(&["qa", "mouse-up"]);
    run.settled();
    let state = run.report();
    assert_eq!(state["reader"]["selection"]["selected"], false, "{state}");
    assert_eq!(state["app"]["pages"]["highlight_layers"], 0, "{state}");
    assert_eq!(menu_item(&state, "edit.copy")["enabled"], false, "{state}");
    // Select All on nothing but a scan: it can be chosen, and it says there's nothing, and the clipboard's left alone.
    run.ok(&["qa", "menu", "edit.select_all"]);
    run.settled();
    copy_now(&run);
    let after = wait_for(&run, |state| state["app"]["copy_notice"].is_object());
    assert!(after["app"]["copy_notice"]["text"].as_str().unwrap().contains("no text"), "{after}");
    assert_eq!(copied(&run), "");
}

/// Failure: hidden text reaches the clipboard, and nothing says some was left out, or the notice sits on the page. Where it is, and what it says, is what the reader goes by.
#[test]
fn hidden_text_stays_off_the_clipboard_and_a_quiet_notice_says_so() {
    let Some(run) = Run::start() else { return };
    let file = TempPdf::new("hidden.pdf", &pages(&[Page::new(hidden_text_page())]));
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "menu", "edit.select_all"]);
    run.settled();
    copy_now(&run);
    let text = copied(&run);
    assert!(text.contains("VISIBLE-SENTINEL") && !text.contains("HIDDEN-"), "{text}");
    let state = wait_for(&run, |state| state["app"]["copy_notice"].is_object());
    assert!(state["app"]["copy_notice"]["text"].as_str().unwrap().contains("hidden"), "{state}");
    assert_eq!(state["app"]["copy_notice"]["frame_ok"], true, "{state}");
    assert!(state["app"]["copy_notice"]["button"].is_null(), "nothing to choose: {state}");
}

/// Failure: a drag past the bottom of the window doesn't scroll, or keeps scrolling after the button's let go, with a timer left running. A selection longer than a window can't be made without this.
#[test]
fn a_drag_past_the_edge_scrolls_until_it_is_let_go() {
    let Some(run) = Run::start() else { return };
    let file = long_document(20);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let first = place_on(&run, 1, 0, 0.0);
    run.ok(&["qa", "mouse-down", &first]);
    let before = run.report()["app"]["pages"]["scroll_y"].as_f64().unwrap();
    // A long way down the document, which is past the bottom of the view.
    let dragging = run.ok(&["qa", "mouse-drag", "10:100,100"]);
    assert_eq!(dragging["pages"]["autoscrolling"], true, "{dragging}");
    let moved = wait_for(&run, |state| state["app"]["pages"]["scroll_y"].as_f64().unwrap() > before + 50.0);
    assert_eq!(moved["reader"]["selection"]["dragging"], true, "{moved}");
    let done = run.ok(&["qa", "mouse-up"]);
    assert_eq!(done["pages"]["autoscrolling"], false, "{done}");
    let stopped = done["pages"]["scroll_y"].as_f64().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(run.report()["app"]["pages"]["scroll_y"].as_f64().unwrap(), stopped, "nothing's scrolling now");
    assert_eq!(run.report()["reader"]["selection"]["selected"], true);
}

/// Failure: text read for a selection stays behind when the tab goes or the window closes, and a cold tab's cost grows by a page's text, or a highlight comes back on the wrong document. A selection is what's on screen.
#[test]
fn another_tab_or_a_closed_window_holds_no_text() {
    let Some(run) = Run::start() else { return };
    let (one, two) = (TempPdf::new("one.pdf", &pages(&[Page::new(text_lines(&["First document's words"]))])), TempPdf::new("two.pdf", &pages(&[Page::new(text_lines(&["Second document's words"]))])));
    run.ok(&["open", one.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "mouse-down", &format!("{}:2", place_on(&run, 1, 0, 0.5))]);
    run.ok(&["qa", "mouse-up"]);
    run.settled();
    assert!(run.report()["reader"]["selection"]["text_bytes_held"].as_u64().unwrap() > 0);
    run.ok(&["open", two.0.to_str().unwrap()]);
    let moved = wait_for(&run, |state| state["reader"]["selection"]["text_pages_held"] == 0);
    assert_eq!(moved["reader"]["selection"]["selected"], false, "{moved}");
    run.settled();
    assert_eq!(run.report()["app"]["pages"]["highlight_layers"], 0);
    run.ok(&["qa", "mouse-down", &format!("{}:2", place_on(&run, 1, 0, 0.5))]);
    run.ok(&["qa", "mouse-up"]);
    run.settled();
    assert_eq!(run.report()["app"]["pages"]["highlight_layers"], 1);
    run.ok(&["qa", "window", "close"]);
    assert_eq!(run.report()["reader"]["selection"]["text_pages_held"], 0, "a closed window holds none");
    run.ok(&["qa", "window", "open"]);
    run.settled();
    let back = run.report();
    assert_eq!(back["reader"]["selection"]["selected"], true, "the selection's still there to come back to: {back}");
    assert_eq!(back["app"]["pages"]["highlight_layers"], 1, "and it shows: {back}");
}

/// Failure: the selection script an agent runs goes out of date, as a script nothing runs does.
#[test]
fn the_selection_script_passes() {
    let Some(run) = Run::start() else { return };
    let file = TempPdf::new("script.pdf", &pages(&[Page::new(text_lines(&["Words at the top of the first page", "with more under them"])), Page::new(text_lines(&["Words at the top of the second page"]))]));
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app/qa/selection.qa");
    let output = run.state.command().args(["qa", "run", script.to_str().unwrap(), "--json"]).env("RECTO_QA_PDF", &file.0).output().unwrap();
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["data"]["passed"], true, "{reply}");
}

/// A file anyone can read and that asks not to be copied.
fn restricted_pdf() -> TempPdf {
    use lopdf::encryption::{EncryptionState, EncryptionVersion, Permissions};
    let bytes = pages(&[Page::new(text_lines(&["Words that ask not to be copied"]))]);
    let mut document = lopdf::Document::load_mem(&bytes).unwrap();
    document.trailer.set("ID", lopdf::Object::Array(vec![lopdf::Object::string_literal("0123456789abcdef"), lopdf::Object::string_literal("0123456789abcdef")]));
    let state = EncryptionState::try_from(EncryptionVersion::V2 { document: &document, owner_password: "owner", user_password: "", key_length: 128, permissions: Permissions::all() - Permissions::COPYABLE }).unwrap();
    document.encrypt(&state).unwrap();
    let mut out = Vec::new();
    document.save_to(&mut out).unwrap();
    TempPdf::new("restricted.pdf", &out)
}

/// Failure: a file that asks not to be copied is copied with nothing said, or refuses with nothing to do about it. The coordinator's default, until the owner says: a small notice, and "Copy anyway", through the button itself.
#[test]
fn a_copy_restricted_file_offers_copy_anyway() {
    let Some(run) = Run::start() else { return };
    let file = restricted_pdf();
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "mouse-down", &format!("{}:3", place_on_anyway(&run))]);
    run.ok(&["qa", "mouse-up"]);
    run.settled();
    assert_eq!(run.report()["reader"]["selection"]["selected"], true, "selecting isn't what's restricted");
    copy_now(&run);
    let asked = wait_for(&run, |state| state["app"]["copy_notice"].is_object());
    assert!(asked["app"]["copy_notice"]["text"].as_str().unwrap().contains("permissions"), "{asked}");
    assert_eq!(asked["app"]["copy_notice"]["button"], "Copy anyway", "{asked}");
    assert_eq!(copied(&run), "", "nothing's on the clipboard yet");
    run.ok(&["qa", "notice-button"]);
    run.settled();
    assert_eq!(copied(&run), "Words that ask not to be copied");
    let (reply, status) = run.state.json(&["qa", "notice-button"]);
    assert_eq!(status, exit::FAILED, "no notice now: {reply}");
}

/// The middle of the first line. `recto select` gives rectangles for a restricted file too, and no text.
fn place_on_anyway(run: &Run) -> String {
    place_on(run, 1, 0, 0.5)
}

/// Failure: `qa menu` chooses an item the real menu has greyed out, which a person can't do, so what's tested is a path nobody takes: `qa menu edit.copy` with nothing selected said it was copying, for good.
#[test]
fn a_greyed_out_menu_item_cant_be_chosen_by_qa_either() {
    let Some(run) = Run::start() else { return };
    let file = long_document(3);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let (reply, status) = run.state.json(&["qa", "menu", "edit.copy"]);
    assert_eq!(status, exit::FAILED, "it's greyed out: {reply}");
    assert_eq!(run.report()["reader"]["selection"]["copying"], false);
    run.ok(&["qa", "menu", "edit.select_all"]);
    run.settled();
    run.ok(&["qa", "menu", "edit.copy"]);
    run.settled();
    assert_eq!(run.report()["reader"]["selection"]["copying"], false, "and it finishes");
}

/// Failure: an unsaved edit leaves a selection on what's now another page, a copy gives what's no longer there, or the wrong page's words come off the clipboard. A page deleted is pages moved, and what was selected on them is gone. A word then comes from where it now is.
#[test]
fn a_selection_during_an_unsaved_edit_follows_what_is_on_screen() {
    let Some(run) = Run::start() else { return };
    let file = TempPdf::new("edits.pdf", &pages(&[Page::new(text_lines(&["Alpha page words"])), Page::new(text_lines(&["Bravo page words"])), Page::new(text_lines(&["Charlie page words"]))]));
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "zoom", "0.5"]);
    run.settled();
    let line = "1:90,72:2";
    run.ok(&["qa", "mouse-down", &line.replacen('1', "2", 1)]);
    run.ok(&["qa", "mouse-up"]);
    run.settled();
    assert_eq!(run.report()["reader"]["selection"]["selected"], true);
    run.ok(&["page", "delete", "1"]);
    let moved = wait_for(&run, |state| state["reader"]["pages"] == 2);
    assert_eq!(moved["reader"]["selection"]["selected"], false, "{moved}");
    run.settled();
    assert_eq!(run.report()["app"]["pages"]["highlight_layers"], 0);
    // Bravo is the first page now.
    run.ok(&["qa", "mouse-down", line]);
    run.ok(&["qa", "mouse-up"]);
    run.settled();
    copy_now(&run);
    assert_eq!(copied(&run), "Bravo");
    assert_eq!(run.ok(&["copy", "--all"])["untrusted"]["text"], "Bravo page words\nCharlie page words", "what's on screen, in that order");
    // A rotation that isn't saved isn't drawn, so a selection on that page is as it was.
    run.ok(&["page", "rotate", "1", "--by", "90"]);
    run.settled();
    let turned = run.report();
    assert_eq!(turned["reader"]["selection"]["selected"], true, "{turned}");
    copy_now(&run);
    assert_eq!(copied(&run), "Bravo");
    run.ok(&["undo"]);
    run.ok(&["undo"]);
    let back = wait_for(&run, |state| state["reader"]["pages"] == 3);
    assert_eq!(back["reader"]["selection"]["selected"], false, "the pages moved back, so that's another page: {back}");
}

/// Failure: an agent's use of `--anyway` leaves no mark. Owner, 2026-11-07: the agent keeps it, and each use goes in the activity log. Through the real server, at the level both commands need, and read back by `recto log`. Text from the document never goes in.
#[test]
fn each_use_of_copy_anyway_is_in_the_activity_log() {
    let Some(run) = Run::start() else { return };
    let file = restricted_pdf();
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.state.set_level(Level::ReadOnly);
    let uses = |run: &Run| run.ok(&["log", "--last", "200"])["entries"].as_array().unwrap().iter().filter(|row| row["details"]["anyway"] == true).count();
    let before = uses(&run);
    let (refused, status) = run.state.json(&["copy", "1"]);
    assert_eq!(status, exit::PROTECTED, "{refused}");
    assert_eq!(uses(&run), before, "nothing's been gone past");
    for times in 1..=2 {
        let copied = run.ok(&["copy", "1", "--anyway"]);
        assert_eq!(copied["untrusted"]["text"], "Words that ask not to be copied");
        assert_eq!(uses(&run), before + times, "each use");
    }
    let shown = run.ok(&["select", "1", "--anyway"]);
    assert_eq!(shown["untrusted"]["text"], "Words that ask not to be copied");
    assert_eq!(uses(&run), before + 3, "`select` gives the text too");
    let log = run.ok(&["log", "--last", "200"]).to_string();
    assert!(!log.contains("Words that ask"), "no document text: {log}");
}
