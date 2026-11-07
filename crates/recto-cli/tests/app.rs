//! The real app in a quiet run, through `recto`, as an agent reaches it. Off every screen, never active, on temp state: the owner keeps working while these run.
//!
//! The app has to be built, and these skip if it isn't, so `tools/check.sh --quick` stays fast and works for anyone without it. `--full` builds it and sets `RECTO_REQUIRE_APP`, so there a missing app is a failure. Asserts are on what the app reports. Pictures are for people.

mod common;

use common::{State, bin};
use recto_agent::{Level, exit};
use recto_testpdf::{Object, Page, TempPdf, build, form_bomb, pages, text_lines};
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

/// Failure: the appearance, paper or focus don't reach what's drawn, or a change draws nothing new. Read back from the app and from the tiles it attached.
#[test]
fn dark_mode_paper_and_focus_change_what_is_drawn() {
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
    run.ok(&["view", "--focus", "on"]);
    let focused = wait_for(&run, |state| state["app"]["focus"] == true);
    assert_eq!(focused["app"]["sidebar_visible"], false, "{focused}");
    assert_eq!(focused["app"]["agent_icon"], true, "kept");
    assert!(focused["app"]["place_label"].as_str().is_some_and(|place| place.contains("of 3")), "the page number stays: {focused}");
    run.settled();
    let wider = run.report()["reader"]["fit"].as_f64().unwrap();
    run.ok(&["view", "--focus", "off"]);
    let back = wait_for(&run, |state| state["app"]["sidebar_visible"] == true);
    assert!(back["reader"]["fit"].as_f64().unwrap() < wider, "focus view's page is wider: {back}");
}

/// Failure: closing the window keeps tiles and a worker, so it's over budget, or the app quits, or the tabs are lost. The red button's path, and back. The agent's socket stays.
#[test]
fn closing_the_window_frees_tiles_and_keeps_the_session() {
    let Some(run) = Run::start() else { return };
    let file = long_document(2);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    assert!(run.report()["reader"]["tiles"]["attached"].as_u64().unwrap() >= 1);
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
    assert!(back["reader"]["tiles"]["attached"].as_u64().unwrap() >= 1, "{back}");
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
    let footprint = || run.report()["memory"]["footprint_bytes"].as_u64().unwrap();
    // The first few crossings make what's reused from then on.
    for page in ["2", "1", "2", "1"] {
        run.ok(&["goto", page]);
        run.settled();
    }
    let before = footprint();
    for round in 0..400 {
        run.ok(&["goto", if round % 2 == 0 { "2" } else { "1" }]);
    }
    run.settled();
    let grew = footprint().saturating_sub(before);
    assert!(grew < 1024 * 1024, "the app grew by {grew} bytes over 400 page crossings");
}

/// Failure: a long outline pushes the other tabs and the footer off the sidebar, the reason the owner asked for this, at a normal height or a short one. Or the rows of a long outline are all built. Or reading drags the contents back from where someone has scrolled them, or they lose the place while reading on.
#[test]
fn a_long_outline_scrolls_by_itself_and_keeps_the_tabs_on_screen() {
    let Some(run) = Run::start() else { return };
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
    assert_eq!(contents["tabs_scroll"], false, "five tabs fit: {contents}");
    assert!(contents["height"].as_f64().unwrap() + 5.0 * 32.0 <= contents["list_height"].as_f64().unwrap() + 0.5, "{contents}");
    assert_eq!(contents["tab_rows_on_screen"], 5, "all the tabs show: {contents}");
    assert_eq!(contents["footer_on_screen"], true);
    assert!(contents["row_views"].as_u64().unwrap() < 80, "views for what's on screen, not 2,000: {contents}");
    // A short window.
    let short = run.ok(&["qa", "resize", "900x420"]);
    assert_eq!(short["contents"]["tab_rows_on_screen"], 5, "{short}");
    assert!(short["contents"]["height"].as_f64().unwrap() + 5.0 * 32.0 <= short["contents"]["list_height"].as_f64().unwrap() + 0.5, "{short}");
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
    assert!(contents["moves"].as_u64().unwrap() >= 1, "they did follow: {sweep}");
    // Reposition, then rest: 6 sections a second past a dead zone of about 12 rows is a move every 2 s or so. Well below one a section.
    assert!(contents["moves"].as_u64().unwrap() <= 6, "{sweep}");
    assert!(contents["rest_s"]["median"].as_f64().unwrap() >= 0.4, "{sweep}");
    assert!(contents["steps_with_place_in_view"].as_f64().unwrap() >= steps * 0.95, "{sweep}");
    let settled = wait_for(&run, |state| state["app"]["contents"]["frames_running"] == false);
    assert_eq!(settled["app"]["contents"]["moving"], false);
    // With nothing to do there's nothing to wake for.
    let before = recto_platform::local::process_counters(run.pid).unwrap();
    std::thread::sleep(std::time::Duration::from_secs(2));
    let after = recto_platform::local::process_counters(run.pid).unwrap();
    assert!(after.wakeups.saturating_sub(before.wakeups) <= 8, "{} wake-ups in 2 s at rest", after.wakeups.saturating_sub(before.wakeups));
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
