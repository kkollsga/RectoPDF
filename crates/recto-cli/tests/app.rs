//! The real app in a quiet run, through `recto`, as an agent reaches it. Off every screen, never active, on temp state: the owner keeps working while these run.
//!
//! The app has to be built, and these skip if it isn't, so `tools/check.sh --quick` stays fast and works for anyone without it. `--full` builds it and sets `RECTO_REQUIRE_APP`, so there a missing app is a failure. Asserts are on what the app reports. Pictures are for people.

mod common;

use common::app::{Run, app_path, chaptered, numbered, still_running, wait_for};
use common::{State, bin};
use recto_agent::{Level, exit};
use recto_testpdf::{Object, Page, TempPdf, build, form_bomb, hidden_text_page, pages, text_lines};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Command;

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
    let file = numbered(4);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let later = run.report()["app"].clone();
    assert_eq!((later["is_active"].clone(), later["activation"].clone(), later["window"]["is_visible"].clone()), (json!(false), json!("prohibited"), json!(false)), "drawing a page changes none of that: {later}");
}

/// Failure: the app and `recto` have a session each, so an agent sees nothing the reader does, or moves nothing the reader sees. One session, in both directions, over the real socket.
#[test]
fn recto_acts_on_the_running_app_and_sees_what_it_does() {
    let Some(run) = Run::start() else { return };
    let file = numbered(30);
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
    let file = numbered(4);
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
    let (first, second) = (outlined(), numbered(2));
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

/// Failure: the sidebar's offer of generated contents doesn't reach the screen for a file with none, a click on it doesn't make them, or the generated list shows with nothing to say it's generated, or with a heading no one can see. The click is what `qa click-row note` calls, the same method the note's own click calls.
#[test]
fn the_sidebar_offers_generated_contents_and_labels_them() {
    let Some(run) = Run::start() else { return };
    let file = TempPdf::new("headings.pdf", &recto_testpdf::headings_document());
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let offered = run.report();
    assert_eq!(offered["app"]["contents"]["note"], "Generate contents", "{offered}");
    assert!(offered["app"]["contents"]["note_height"].as_f64().unwrap() > 0.0);
    run.ok(&["qa", "click-row", "note"]);
    // The helper is at background priority, which a busy Mac can starve, so the wait is the agent's own, up to ten minutes.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
    loop {
        assert!(std::time::Instant::now() < deadline, "they weren't made in ten minutes");
        let (done, code) = run.state.json(&["wait", "contents-generated", "--timeout", "120s"]);
        if code != recto_agent::exit::TIMED_OUT {
            assert_eq!(code, 0, "{done}");
            break;
        }
    }
    let shown = wait_for(&run, |state| state["app"]["contents"]["note"] == "Generated contents");
    assert_eq!(shown["app"]["contents"]["count"], 6, "{shown}");
    let sidebar = run.ok(&["sidebar"]);
    let titles: Vec<&str> = sidebar["rows"].as_array().unwrap().iter().filter(|row| row["kind"] == "contents").map(|row| row["untrusted"]["text"].as_str().unwrap()).collect();
    assert_eq!(titles, ["1 Introduction", "2 Background", "3 Method", "4 Results", "5 Discussion", "6 Conclusion"]);
    assert_eq!(run.ok(&["contents"])["total"], 0, "the file's own are untouched");
}

/// Forty chapters of a long title each, every one with a section under it, on forty pages.
fn sectioned() -> TempPdf {
    let chapters = 40;
    let page = |index: usize| 3 + index * 2;
    let outline = 3 + chapters * 2;
    let mut objects = vec![
        Object::Dict(format!("<< /Type /Catalog /Pages 2 0 R /Outlines {outline} 0 R >>")),
        Object::Dict(format!("<< /Type /Pages /Kids [{}] /Count {chapters} >>", (0..chapters).map(|index| format!("{} 0 R", page(index))).collect::<Vec<_>>().join(" "))),
    ];
    for index in 0..chapters {
        objects.push(Object::Dict(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R >>", page(index) + 1)));
        objects.push(Object::Stream { dict: String::new(), data: format!("BT /F1 12 Tf 72 700 Td (Page {index}) Tj ET").into_bytes() });
    }
    let chapter = |index: usize| outline + 1 + index * 2;
    objects.push(Object::Dict(format!("<< /Type /Outlines /First {} 0 R /Last {} 0 R /Count {chapters} >>", chapter(0), chapter(chapters - 1))));
    for index in 0..chapters {
        let links = format!("{}{}", if index > 0 { format!(" /Prev {} 0 R", chapter(index - 1)) } else { String::new() }, if index + 1 < chapters { format!(" /Next {} 0 R", chapter(index + 1)) } else { String::new() });
        objects.push(Object::Dict(format!("<< /Title (Chapter {index} with a title long enough to be shortened in the sidebar) /Parent {outline} 0 R{links} /First {0} 0 R /Last {0} 0 R /Count 1 /Dest [{1} 0 R /Fit] >>", chapter(index) + 1, page(index))));
        objects.push(Object::Dict(format!("<< /Title (Section {index}.1) /Parent {} 0 R /Dest [{} 0 R /Fit] >>", chapter(index), page(index))));
    }
    TempPdf::new("sectioned.pdf", &build(&objects))
}

/// Failure: a section's fold doesn't fold it, the full name of a shortened entry waits on a timer or comes in another colour than its row, or the sidebar's thumb is missing while the contents scroll, or sits over their text. Owner, 2026-11-07: "there should be a collapse toggle button for sections with sub chapters on the left side of the headers", "The hover delay before showing the full toc name makes in not intuitive", and "The sidebar thumb should always be visible, and lets hide the rail".
#[test]
fn contents_fold_reveal_at_once_and_keep_a_thumb() {
    let Some(run) = Run::start() else { return };
    let file = sectioned();
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let open = run.report();
    // On page 1, chapter 0 is open by following: 40 chapters and its one section.
    assert_eq!(open["app"]["contents"]["count"], 41, "{open}");
    let thumb = &open["app"]["contents"]["thumb"];
    assert_eq!(thumb["shown"], true, "{open}");
    assert!(thumb["thumb_width"].as_f64().unwrap() < 7.0, "thinner than the system's: {thumb}");
    assert!(thumb["right_gap"].as_f64().unwrap() >= 2.0, "{thumb}");
    for style in ["legacy", "overlay"] {
        run.ok(&["qa", "scroller-style", style]);
        let state = run.report();
        assert_eq!(state["app"]["contents"]["thumb"]["shown"], true, "{style}: {state}");
        assert_eq!(state["app"]["contents"]["scroller"], false, "no system scroller or track: {style}");
    }
    // Folding chapter 0, then opening chapter 1.
    run.ok(&["qa", "click-row", "fold:1"]);
    wait_for(&run, |state| state["app"]["contents"]["count"] == 40);
    run.ok(&["qa", "click-row", "fold:2"]);
    wait_for(&run, |state| state["app"]["contents"]["count"] == 41);
    let sidebar = run.ok(&["sidebar"]);
    let rows = sidebar["rows"].as_array().unwrap();
    assert_eq!((rows[1]["expanded"].clone(), rows[2]["expanded"].clone(), rows[3]["untrusted"]["text"].clone()), (json!(false), json!(true), json!("Section 1.1")), "{sidebar}");
    // The full name, at once, in the row's own colour.
    run.ok(&["qa", "hover", "2"]);
    let shown = run.ok(&["qa", "reveal", "2"]);
    assert_eq!(shown["reveal"]["shown"], true, "{shown}");
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
    let file = numbered(2);
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
    let file = numbered(3);
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
    let file = numbered(5);
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
    let (file, other) = (numbered(9), TempPdf::new("other.pdf", &pages(&[Page::new(text_lines(&["Another document"]))])));
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
    let file = numbered(4);
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

/// Failure (second review): in a narrower window, a laptop's with the sidebar, the zoom level or the agent icon sits on the page, where the owner's rule has nothing; at any width the window can be, with the sidebar shown or hidden, the icon on or off, single or side by side, both stay clear of every page showing at the fitted zoom.
#[test]
fn the_top_right_corner_stays_off_the_page() {
    let Some(run) = Run::start() else { return };
    let file = numbered(4);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    for layout in ["single", "two-up"] {
        run.ok(&["view", "--layout", layout]);
        for sidebar in ["on", "off"] {
            run.ok(&["view", "--sidebar", sidebar]);
            for icon in ["on", "off"] {
                run.ok(&["view", "--agent-icon", icon]);
                for size in ["700x500", "820x600", "1000x700", "1180x760", "1440x900"] {
                    run.ok(&["qa", "resize", size]);
                    run.settled();
                    let state = run.report();
                    let app = &state["app"];
                    assert_eq!((&app["zoom_level"]["clear_of_pages"], &app["agent_icon_margins"]["clear_of_pages"]), (&json!(true), &json!(true)), "{layout}, sidebar {sidebar}, icon {icon}, {size}: zoom {} icon {}", app["zoom_level"], app["agent_icon_margins"]);
                }
            }
        }
    }
}

/// Failure: the zoom level isn't in the top right just left of the agent icon, in its row, or doesn't take the icon's place at the same margin when the icon's hidden, or the page's scroller runs up beside it; or it says a level the page isn't at, or clicking it doesn't let the reader type a level or fit the page again, or Escape or going elsewhere changes it. Owner, 2026-11-07: "clicking the zoom value enables you to type in zoom level or reset", and 2026-11-08: "The zoom level should be in the upper right of the app (not in the sidebar)". 100% is the page's real size.
#[test]
fn the_zoom_level_shows_and_takes_a_typed_level() {
    let Some(run) = Run::start() else { return };
    let file = numbered(5);
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
    // Escape, and going elsewhere, leave the zoom as it was.
    run.ok(&["qa", "button", "zoom"]);
    let escaped = run.ok(&["qa", "zoom-escape"]);
    assert_eq!((&escaped["zoom_level"]["editing"], &escaped["zoom_level"]["text"]), (&json!(false), &json!("200%")), "{escaped}");
    run.ok(&["qa", "button", "zoom"]);
    let away = run.ok(&["qa", "focus", "page"]);
    assert_eq!((&away["zoom_level"]["editing"], &away["zoom_level"]["text"]), (&json!(false), &json!("200%")), "clicking away: {away}");
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
    for name in ["command_plus_zooms_in", "command_minus_zooms_out", "command_9_fits", "command_0_is_actual_size", "plain_plus_zooms_in", "plain_minus_zooms_out", "command_scroll_zooms", "command_scroll_keeps_the_point_still", "more_button", "panel_opens", "panel_button_answers", "panel_closes", "sidebar_area_is_on_the_sidebar", "contents_area_is_on_the_contents", "sidebar_button", "zoom_level", "zoom_field", "zoom_reset", "undo_reaches_the_app_with_the_window_key", "redo_reaches_the_app_with_the_window_key", "hover_shows_details", "pointer_finds_a_tab_row", "contents_hover_brings_no_system_scroller", "pointer_finds_the_contents_row", "contents_hover_reveals_the_full_name", "contents_hover_ends", "hover_ends", "reveal_goes_with_the_pointer", "plain_wheel_goes_to_the_scroll_view", "wheel_in_contents_stops_following", "pinch_starts", "pinch_ends", "mouse_reaches_the_selection", "pointer_reaches_the_cursor", "pointer_area_is_on_the_page", "right_click_has_a_menu", "copy_is_grey_with_nothing_selected", "select_all_can_be_chosen", "select_all_key_reaches_the_app", "select_all_selects", "copy_is_enabled_with_something_selected", "copy_key_reaches_the_app", "services_arent_offered_what_they_didnt_ask_for", "services_arent_offered_nothing", "services_write_nothing_with_nothing_selected", "tab_row_click", "contents_row_click", "add_button", "home_button"] {
        assert!(checked.contains_key(name), "{name} wasn't checked: {found}");
    }
}

/// Failure: with a mouse connected, or "always show" set, a Mac's scrollers take their width from the view, and a page fitted to the view's whole width is wider than what's left, so a scroll bar sits under it for good and the vertical reading space goes down by it. Seen in a shot, when one was plugged in. A fitted page fits, by either style.
#[test]
fn a_fitted_page_fits_with_classic_scrollers() {
    let Some(run) = Run::start() else { return };
    let file = numbered(5);
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
    let short = numbered(1);
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
    let file = numbered(60);
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

/// Failure: the owner couldn't see the settings: the panel is there only if it can be opened, keeps to tools, and at rest there's only the button. Paper is in View > Paper (the owner's default of 2026-11-08), which ticks what's showing and changes it through the real menu item, with no re-render for warm paper.
#[test]
fn the_panel_opens_and_does_what_it_says() {
    let Some(run) = Run::start() else { return };
    // In a light window, where the paper with nothing chosen is white, and warm is that with a tint.
    run.ok(&["qa", "appearance", "light"]);
    let file = numbered(5);
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
    assert_eq!(ids, ["tools.snapshot", "rotate.left", "rotate.right"], "tools and nothing else: paper is View's, and zoom is the top right's, by the owner's word");
    assert!(open["app"]["contents"]["list_height"].as_f64().unwrap() < list_at_rest, "it docks in room from the list: {open}");
    assert!(menu_item(&open, "paper.white")["checked"] == true && menu_item(&open, "paper.warm")["checked"] == false, "View > Paper ticks the paper showing: {}", menu_item(&open, "paper.white"));
    let drawn = open["reader"]["counts"]["drawn"].clone();
    run.ok(&["qa", "menu", "paper.warm"]);
    let warm = wait_for(&run, |state| state["app"]["paper"] == "warm" && menu_item(state, "paper.warm")["checked"] == true);
    assert_eq!(warm["app"]["pages"]["tint_layers"].as_u64().map(|layers| layers >= 1), Some(true), "{warm}");
    assert_eq!(warm["reader"]["counts"]["drawn"], drawn, "warm paper draws nothing");
    let ticked: Vec<&str> = ["paper.white", "paper.warm", "paper.dark"].into_iter().filter(|id| menu_item(&warm, id)["checked"] == true).collect();
    assert_eq!(ticked, ["paper.warm"]);
    assert_eq!(run.ok(&["view"])["paper"], "warm", "the same setting `recto view` has");
    // The palette still offers each paper, named to stand alone.
    let found = run.ok(&["palette", "dark paper"]);
    assert_eq!((found["items"][0]["id"].as_str(), found["items"][0]["title"].as_str()), (Some("paper.dark"), Some("Dark Paper")), "{found}");
    run.ok(&["qa", "panel", "toggle"]);
    wait_for(&run, |state| state["app"]["panel"]["open"] == false);
}

/// Failure: the menu bar isn't what Rust lists, or the star's margins aren't the same to the right and above, or Show Agent Icon doesn't hide the star, or isn't remembered, or the page's scroller still runs under where the star is, or the full way with a star there. Owner, 2026-11-07: "To save horizontal space the scrollbar can stop below the agent icon. Add an option in the macos top bar to turn off agent bar (hides the icon), enable + - and command - scroll for zooming".
#[test]
fn the_menu_bar_and_the_agent_icon() {
    let Some(run) = Run::start() else { return };
    let file = numbered(5);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let state = run.report();
    let titles: Vec<&str> = state["app"]["menu_bar"].as_array().unwrap().iter().map(|menu| menu["title"].as_str().unwrap()).collect();
    // The Go menu has Back, Forward and Go to Page…, as SPEC lists the menus.
    assert_eq!(titles, ["RectoPDF", "File", "Edit", "View", "Go", "Tools"], "{state}");
    let view = &state["app"]["menu_bar"][3]["items"];
    let names: Vec<&str> = view.as_array().unwrap().iter().map(|item| item["title"].as_str().unwrap()).collect();
    // Paper is View > Paper, its submenu's items following it (the owner's default of 2026-11-08).
    assert_eq!(names, ["Zoom In", "Zoom Out", "Actual Size", "Fit", "Hide Sidebar", "Paper", "White", "Warm", "Dark", "Single Page", "Side by Side", "Side by Side, First Page Alone", "Show Agent Icon", "Command Palette…"]);
    assert_eq!(view[13]["key"], "k", "⌘K, as SPEC has it");
    assert_eq!(state["app"]["agent_icon"], true);
    // Owner, 2026-11-07: the same margin to the right as above, and less above than the 16 it had.
    let (top, right) = (state["app"]["agent_icon_margins"]["top"].as_f64().unwrap(), state["app"]["agent_icon_margins"]["right"].as_f64().unwrap());
    assert!((top - right).abs() < 0.5 && (8.0..16.0).contains(&top), "top {top}, right {right}");
    assert!(state["app"]["pages"]["scroller_top_inset"].as_f64().unwrap() > 28.0, "below the star: {state}");
    run.ok(&["qa", "menu", "view.agent_icon"]);
    let hidden = wait_for(&run, |state| state["app"]["agent_icon"] == false);
    assert!(hidden["app"]["pages"]["scroller_top_inset"].as_f64().unwrap() < state["app"]["pages"]["scroller_top_inset"].as_f64().unwrap() + 1.0, "no lower than with the star, as the zoom level takes its place: {hidden}");
    assert_eq!(menu_item(&hidden, "view.agent_icon")["checked"], false, "{hidden}");
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
    let file = numbered(5000);
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
    let file = numbered(12);
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

/// Failure: a long outline pushes the other tabs and the bottom bar off the sidebar, the reason the owner asked for this, at a normal height or a short one. Or the rows of a long outline are all built. Or reading drags the contents back from where someone has scrolled them, or they lose the place while reading on.
#[test]
fn a_long_outline_scrolls_by_itself_and_keeps_the_tabs_on_screen() {
    let Some(run) = Run::start() else { return };
    run.ok(&["qa", "scroller-style", "overlay"]);
    let long = outlined_with(2000);
    let others: Vec<TempPdf> = (0..4).map(|_| numbered(1)).collect();
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
    assert_eq!(contents["panel_button_on_screen"], true);
    assert!(contents["height"].as_f64().unwrap() <= contents["list_height"].as_f64().unwrap() * 0.6 + 0.5, "it stops at the maximum: {contents}");
    assert!(contents["row_views"].as_u64().unwrap() < 80, "views for what's on screen, not 2,000: {contents}");
    // A short window.
    let short = run.ok(&["qa", "resize", "900x420"]);
    assert!(short["contents"]["height"].as_f64().unwrap() <= short["contents"]["list_height"].as_f64().unwrap() * 0.6 + 0.5, "the same maximum, and the tabs scroll in what's left: {short}");
    assert_eq!(short["contents"]["panel_button_on_screen"], true, "{short}");
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
    // Failure: the thumb comes and goes with each reposition, where the owner wants it always there and slim (2026-11-07: "The sidebar thumb should always be visible, and lets hide the rail").
    assert_eq!(again["app"]["contents"]["thumb"]["shown"], true, "{again}");
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
/// Failure: a chapter's change is published once and the next look republishes "nothing changed", and the host reads only the latest, so the table keeps its old row count, and later row changes go at the wrong places. Or a change is applied twice. The table must hold exactly what the model says through every kind of move, and reading forward never moves the contents back.
#[test]
fn the_contents_table_follows_chapters_opening_and_closing() {
    let Some(run) = Run::start() else { return };
    let (chapters, sections) = (40, 8);
    let file = chaptered("chapters", chapters, sections);
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

/// Failure: `qa wait-settled` comes back while the sidebar's contents are still fading, sliding or drifting after a change of chapter, so a shot taken straight after catches two contents rows drawn over each other. Settled takes in every host animation that changes what's shown, and still comes back, and `qa shot` waits for the same.
#[test]
fn wait_settled_waits_for_the_sidebars_animations() {
    let Some(run) = Run::start() else { return };
    let sections = 8;
    let file = chaptered("chapters", 40, sections);
    run.ok(&["qa", "reduce-motion", "off"]);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let mut waited = 0;
    for chapter in [1, 2, 3, 10, 9, 30, 0] {
        run.ok(&["goto", &(chapter * (sections + 1) + 1).to_string()]);
        let settled = run.ok(&["qa", "wait-settled", "--timeout", "30s"]);
        let state = run.report();
        assert_eq!(state["app"]["animations"], json!([]), "chapter {chapter}: settled, and still moving: {settled} {}", state["app"]["contents"]);
        assert_eq!(state["app"]["contents"]["stale_rows"], 0, "chapter {chapter}: {}", state["app"]["contents"]);
        waited += settled["animations_waited_ms"].as_u64().unwrap();
    }
    assert!(waited > 0, "a change of chapter moves the contents, so there was something to wait for");
    // A shot waits for the same settled by itself.
    run.ok(&["goto", &(20 * (sections + 1) + 1).to_string()]);
    let shot = run.ok(&["qa", "shot", "--out", run.state.root.join("settled.png").to_str().unwrap()]);
    assert_eq!(shot["settled"], true, "{shot}");
    assert_eq!(run.report()["app"]["animations"], json!([]), "a shot is taken with nothing moving");
}

/// Failure: closing a warm tab leaves its document, tiles and worker held until the next switch of tab, which may never come.
#[test]
fn closing_a_warm_tab_lets_go_of_it() {
    let Some(run) = Run::start() else { return };
    let (one, two) = (numbered(2), TempPdf::new("second.pdf", &pages(&[Page::new(text_lines(&["Second"]))])));
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
    let file = numbered(2);
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
    let file = numbered(2);
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
    let short = numbered(1);
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
    let file = numbered(20);
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

/// Failure: the app's ⌘C over a page break copies the running head and page number into the body text, or a running head selected by itself copies nothing, as `recto copy` was proved not to: the app goes through the same rule, through what a real press, drag and release call.
#[test]
fn the_chrome_script_passes() {
    let Some(run) = Run::start() else { return };
    let report: Vec<Page> = (1..=4)
        .map(|page| Page::new(format!("BT /F1 9 Tf 72 750 Td (Annual report 2024) Tj ET\nBT /F1 11 Tf 14 TL 72 690 Td (Body of page {page}, first line.) Tj T* (Body of page {page}, second line.) Tj ET\nBT /F1 9 Tf 540 30 Td ({page}) Tj ET\n").into_bytes()))
        .collect();
    let file = TempPdf::new("report.pdf", &pages(&report));
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app/qa/chrome.qa");
    let output = run.state.command().args(["qa", "run", script.to_str().unwrap(), "--json"]).env("RECTO_QA_PDF", &file.0).output().unwrap();
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["data"]["passed"], true, "{reply}");
}

/// A file anyone can read and that asks not to be copied.
#[allow(clippy::disallowed_methods)] // A test fixture built with lopdf's eager writer, which alone encrypts or rebuilds; never over a user's file.
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
    let file = numbered(3);
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

/// Failure (review): the window's Save, the close prompt and Save As act on the wrong tab, or nothing. A click dropped because the session was busy; "Save" in
/// the prompt then refusing to close silently over undone steps; an agent's `tap` while Save As's sheet is up saving the other document. Driven through the
/// QA verbs, which call what the window does: the Save icon saves the tab showing; the prompt's Don't Save closes without writing; Save As's sheet names the
/// tab's file and its answer saves that tab; and with only undone steps, the prompt offers Close and says they'll go.
#[test]
fn the_windows_save_close_prompt_and_save_as_act_on_their_own_tab() {
    let Some(run) = Run::start() else { return };
    let first = numbered(3);
    let second = numbered(2);
    let original = std::fs::read(&first.0).unwrap();
    run.ok(&["open", second.0.to_str().unwrap()]);
    run.ok(&["open", first.0.to_str().unwrap()]);
    run.settled();
    // The Save icon shows with an edit, saves the tab showing, and goes.
    run.ok(&["page", "rotate", "1", "--by", "90"]);
    let edited = wait_for(&run, |state| state["app"]["save_button"]["visible"] == true);
    assert_eq!(edited["app"]["document_edited"], true);
    run.ok(&["qa", "button", "save"]);
    wait_for(&run, |state| state["app"]["save_button"]["visible"] == false && state["app"]["document_edited"] == false);
    let saved = std::fs::read(&first.0).unwrap();
    assert!(saved.starts_with(&original) && saved.len() > original.len(), "the tab showing was saved");

    // Don't Save closes without writing.
    run.ok(&["page", "rotate", "1", "--by", "90"]);
    wait_for(&run, |state| state["app"]["document_edited"] == true);
    run.ok(&["qa", "menu", "file.close"]);
    let asked = wait_for(&run, |state| state["app"]["sheet"]["kind"] == "close_prompt");
    assert_eq!(asked["app"]["sheet"]["buttons"], json!(["Save", "Cancel", "Don't Save"]));
    run.ok(&["qa", "sheet-answer", "third"]);
    wait_for(&run, |state| state["app"]["sheet"].as_object().is_none_or(|sheet| sheet.is_empty()));
    let (tabs, _) = run.state.json(&["tabs"]);
    assert_eq!(tabs["data"]["count"], 1, "closed: {tabs}");
    assert_eq!(std::fs::read(&first.0).unwrap(), saved, "and nothing was written");

    // Save As's sheet names the tab's file, and its answer saves that tab: the system's sheet can't be typed into here, so `qa save-as` gives the answer it would.
    run.ok(&["open", first.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["page", "rotate", "1", "--by", "180"]);
    wait_for(&run, |state| state["app"]["document_edited"] == true);
    run.ok(&["qa", "menu", "file.save_as"]);
    let sheet = wait_for(&run, |state| state["app"]["sheet"]["kind"] == "save_panel");
    assert!(sheet["app"]["sheet"]["name"].as_str().unwrap().ends_with(" copy.pdf"), "{}", sheet["app"]["sheet"]);
    run.ok(&["qa", "sheet-answer", "cancel"]);
    let copy = first.0.with_file_name("first-copy.pdf");
    run.ok(&["qa", "save-as", copy.to_str().unwrap()]);
    wait_for(&run, |state| state["app"]["document_edited"] == false);
    assert!(std::fs::read(&copy).unwrap().starts_with(&saved), "the copy is of the tab it was for");

    // Only undone steps: nothing to save, so the prompt offers Close, says the steps go, and closing lets them go.
    // The owner's steps, as the app's own tools make them: an agent's undone steps are its own to lose, and closing doesn't ask about those.
    run.ok(&["qa", "owner-edit", "rotate:1"]);
    run.ok(&["qa", "owner-edit", "undo"]);
    wait_for(&run, |state| state["app"]["document_edited"] == false);
    run.ok(&["qa", "menu", "file.close"]);
    let asked = wait_for(&run, |state| state["app"]["sheet"]["kind"] == "close_prompt");
    assert_eq!(asked["app"]["sheet"]["buttons"], json!(["Close", "Cancel"]));
    assert!(asked["app"]["sheet"]["informative"].as_str().unwrap().contains("undid"), "{}", asked["app"]["sheet"]);
    run.ok(&["qa", "sheet-answer", "first"]);
    wait_for(&run, |state| state["app"]["sheet"].as_object().is_none_or(|sheet| sheet.is_empty()));
    let (tabs, _) = run.state.json(&["tabs"]);
    assert_eq!(tabs["data"]["count"], 1, "closed over its undone steps, as it said: {tabs}");

    // The prompt's Save writes the tab it asked about, then closes it, and a watcher hears it was saved before it hears it closed.
    run.ok(&["open", second.0.to_str().unwrap()]);
    let before = std::fs::read(&second.0).unwrap();
    run.ok(&["page", "rotate", "1", "--by", "90"]);
    wait_for(&run, |state| state["app"]["document_edited"] == true);
    let watching = run.state.command().args(["watch", "--events", "tabs,edits", "--timeout", "60s", "--json"]).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().unwrap();
    // It reports only what happens after it starts, so it's let start first.
    std::thread::sleep(std::time::Duration::from_millis(300));
    run.ok(&["qa", "menu", "file.close"]);
    wait_for(&run, |state| state["app"]["sheet"]["kind"] == "close_prompt");
    run.ok(&["qa", "sheet-answer", "first"]);
    wait_for(&run, |state| state["app"]["home"] == true);
    assert!(std::fs::read(&second.0).unwrap().len() > before.len(), "saved before it closed");
    run.ok(&["watch", "stop"]);
    let heard = String::from_utf8(watching.wait_with_output().unwrap().stdout).unwrap();
    let events: Vec<String> = heard.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()).filter_map(|line| line["data"]["event"].as_str().map(str::to_string)).collect();
    let at = |name: &str| events.iter().position(|event| event == name).unwrap_or_else(|| panic!("no {name} in {events:?}"));
    assert!(at("saved") < at("tab_closed"), "saved, then closed: {events:?}");
    let _ = std::fs::remove_file(copy);
}

/// Failure (the owner's bench file, 2026-11-08): a save changes the view. After turning page 1 and saving, the zoom shown went from 128% to 97% and every page shrank, as the saved file's page 1 is landscape and the reopened layout fitted that. The owner's standard is "perfect" state recovery: the zoom, the place down and across, the layout and the selection are exactly as they were, as `qa state` reports them, with the save written.
#[test]
fn a_save_leaves_the_view_as_it_was() {
    let Some(run) = Run::start() else { return };
    let file = numbered(4);
    let original = std::fs::read(&file.0).unwrap();
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["view", "--zoom", "128%"]);
    run.settled();
    run.ok(&["qa", "scroll", "120"]);
    run.settled();
    run.ok(&["qa", "mouse-down", &format!("{}:2", place_on(&run, 1, 0, 0.3))]);
    run.ok(&["qa", "mouse-up"]);
    run.ok(&["page", "rotate", "1", "--by", "90"]);
    wait_for(&run, |state| state["app"]["document_edited"] == true);
    run.settled();
    let view = |state: &Value| {
        let reader = &state["reader"];
        json!({ "zoom_level": state["app"]["zoom_level"]["text"], "fit": reader["fit"], "magnification": reader["magnification"], "page": reader["page"], "scroll_y": reader["scroll_y"], "scroll_x": reader["scroll_x"], "document_height": reader["document_height"], "layout": state["app"]["layout"], "selected": reader["selection"]["selected"], "start": reader["selection"]["start"], "end": reader["selection"]["end"] })
    };
    let before = view(&run.report());
    assert_eq!((&before["zoom_level"], &before["selected"]), (&json!("128%"), &json!(true)), "{before}");
    assert!(before["scroll_y"].as_f64().unwrap() > 0.0, "down the page: {before}");
    run.ok(&["save"]);
    wait_for(&run, |state| state["app"]["document_edited"] == false);
    run.settled();
    let after = view(&run.report());
    assert_eq!(after, before, "the view after the save");
    let saved = std::fs::read(&file.0).unwrap();
    assert!(saved.starts_with(&original) && saved.len() > original.len(), "and it was saved");
}

/// Failure: ⌘K doesn't find a command by a few of its letters, lists what can't be chosen, words a key unlike its menu, can't be driven by the keyboard alone, or runs something other than what's selected; or an agent's `recto palette` ranks differently from what the reader sees.
#[test]
fn the_palette_finds_commands_and_tabs_and_runs_them_by_keyboard() {
    let Some(run) = Run::start() else { return };
    let file = numbered(4);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "menu", "palette.open"]);
    let opened = wait_for(&run, |state| state["app"]["palette"]["open"] == true);
    assert_eq!(opened["app"]["palette"]["field_has_focus"], true, "⌘K gives its field the keyboard");
    assert!(opened["app"]["palette"]["rows"].as_array().unwrap().iter().any(|row| row["id"] == "tab:1"), "the open tab is listed: {}", opened["app"]["palette"]);
    let typed = run.ok(&["qa", "palette-type", "zi"]);
    assert_eq!(typed["palette"]["rows"][0]["id"], "view.zoom_in", "{}", typed["palette"]);
    assert_eq!(typed["palette"]["rows"][0]["shortcut"], "⌘+", "the menu's own key");
    let agent = run.ok(&["palette", "zi"]);
    assert_eq!(agent["items"][0]["id"], "view.zoom_in", "an agent sees the same order: {agent}");
    run.ok(&["qa", "palette-type", "side"]);
    let moved = run.ok(&["qa", "palette-key", "down"]);
    let second = moved["palette"]["rows"][1]["id"].as_str().unwrap().to_string();
    assert_eq!(moved["palette"]["selected"], 1);
    run.ok(&["qa", "palette-key", "return"]);
    run.settled();
    let ran = run.report();
    assert_eq!(ran["app"]["palette"]["open"], false, "it closes once it runs");
    assert_eq!(format!("layout.{}", ran["app"]["layout"].as_str().unwrap().replace('-', "_")), second, "what was selected ran");
    // What can't be chosen now isn't listed.
    let agent = run.ok(&["palette", "copy"]);
    assert!(!agent["items"].as_array().unwrap().iter().any(|item| item["id"] == "edit.copy"), "nothing's selected, so Copy can't be chosen: {agent}");
    run.ok(&["qa", "menu", "palette.open"]);
    run.ok(&["qa", "palette-key", "escape"]);
    assert_eq!(run.report()["app"]["palette"]["open"], false);
    let refused = run.state.json(&["palette", "zzzzqqq", "--run"]);
    assert_eq!(refused.1, exit::NOT_FOUND, "{}", refused.0);
    // A tab's name is the file's words.
    let tabs = run.ok(&["palette", "long"]);
    let tab = tabs["items"].as_array().unwrap().iter().find(|item| item["id"] == "tab:1").unwrap();
    assert!(tab["untrusted"]["title"].is_string() && tab.get("title").is_none(), "{tab}");
}

/// Failure: a palette row that leaves the host something to do (Settings, the Open panel, About, Save As, Find's field) does nothing when run with Return, because what's left for the host never reaches it.
#[test]
fn a_palette_row_does_what_the_host_is_left_to_do() {
    let Some(run) = Run::start() else { return };
    run.ok(&["qa", "menu", "palette.open"]);
    wait_for(&run, |state| state["app"]["palette"]["open"] == true);
    let typed = run.ok(&["qa", "palette-type", "settings"]);
    assert_eq!(typed["palette"]["rows"][0]["id"], "app.settings", "{}", typed["palette"]);
    run.ok(&["qa", "palette-key", "return"]);
    let shown = wait_for(&run, |state| state["app"]["settings"]["open"] == true);
    assert_eq!(shown["app"]["palette"]["open"], false);
}

/// Failure: Find Next run from the palette with Find closed goes nowhere the reader can see: Find doesn't open, or its field doesn't take the keyboard the palette had, so typing goes nowhere; or `recto palette "find next" --run` needs Full though `recto find --next` needs only Control.
#[test]
fn find_next_from_the_palette_opens_find_with_its_field_focused() {
    let Some(run) = Run::start() else { return };
    let file = numbered(4);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "menu", "find.open"]);
    run.ok(&["qa", "find-type", "words"]);
    run.ok(&["qa", "find-key", "escape"]);
    wait_for(&run, |state| state["app"]["find"]["open"] == false);
    run.ok(&["qa", "menu", "palette.open"]);
    let typed = run.ok(&["qa", "palette-type", "find next"]);
    assert_eq!(typed["palette"]["rows"][0]["id"], "find.next", "{}", typed["palette"]);
    run.ok(&["qa", "palette-key", "return"]);
    let found = wait_for(&run, |state| state["app"]["find"]["open"] == true && state["app"]["find"]["field_has_focus"] == true);
    assert_eq!((found["app"]["palette"]["open"].as_bool(), found["app"]["focus_view"].as_str()), (Some(false), Some("find")), "{}", found["app"]["find"]);
    assert_eq!(found["app"]["find"]["untrusted"]["field"], "words", "with what was typed last: {}", found["app"]["find"]);
    run.state.set_level(Level::Control);
    let stepped = run.ok(&["palette", "find next", "--run"]);
    assert_eq!(stepped["ran"]["id"], "find.next", "at Control, as `recto find --next`: {stepped}");
}

/// Failure: ⌘K doesn't offer files opened lately, or opens another file than the row showed; a file in Open Recent named like a command ("Close") is listed as that command; an agent below `recto recent`'s level sees files opened lately; or an agent sees anything of a tab opened with a password the person typed, whose name may have been read with it.
#[test]
fn the_palette_offers_recent_files_and_keeps_protected_tabs_from_agents() {
    let Some(run) = Run::start() else { return };
    let closed = TempPdf::new("Close.pdf", &pages(&[Page::new(text_lines(&["A file named like a command"]))]));
    let reading = numbered(3);
    run.ok(&["open", closed.0.to_str().unwrap()]);
    run.ok(&["close"]);
    run.ok(&["open", reading.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "menu", "palette.open"]);
    let typed = run.ok(&["qa", "palette-type", "Close"]);
    let rows = typed["palette"]["rows"].as_array().unwrap().clone();
    assert!(rows.iter().all(|row| !row["id"].as_str().unwrap().starts_with("recent.")), "Open Recent's items aren't commands: {rows:?}");
    assert!(rows.iter().any(|row| row["id"] == "file.close"), "{rows:?}");
    assert!(rows[0]["id"].as_str().unwrap().starts_with("recent:"), "the recent file, first among equals after the tabs: {rows:?}");
    run.ok(&["qa", "palette-key", "return"]);
    let opened = wait_for(&run, |state| state["session"]["tabs"].as_array().is_some_and(|tabs| tabs.len() == 2));
    assert!(opened["session"]["tabs"].as_array().unwrap().iter().any(|tab| tab["path"].as_str().is_some_and(|path| path.ends_with("Close.pdf"))), "the file the row showed");
    run.ok(&["close"]);
    run.settled();
    let recent_ids = |reply: &Value| reply["items"].as_array().unwrap().iter().filter(|item| item["id"].as_str().unwrap().starts_with("recent:")).count();
    run.state.set_level(Level::ReadOnly);
    assert_eq!(recent_ids(&run.ok(&["palette", "close"])), 0, "files opened lately need `recto recent`'s level");
    run.state.set_level(Level::Control);
    let listed = run.ok(&["palette", "close"]);
    assert_eq!(recent_ids(&listed), 1, "{listed}");
    let ran = run.ok(&["palette", "Close", "--run"]);
    assert!(ran["ran"]["id"].as_str().unwrap().starts_with("recent:"), "{ran}");
    wait_for(&run, |state| state["session"]["tabs"].as_array().is_some_and(|tabs| tabs.len() == 2));
    // A tab opened with the person's password: theirs to see in ⌘K, and nothing of it for an agent.
    run.state.set_level(Level::Full);
    let locked = TempPdf::new("locked.pdf", &common::locked_with("open sesame", 2));
    run.ok(&["qa", "open-file", locked.0.to_str().unwrap()]);
    sheet_of(&run, "password");
    type_password(&run, "open sesame");
    wait_for(&run, |state| state["app"]["sheet"] == json!({}) && state["session"]["tabs"].as_array().is_some_and(|tabs| tabs.len() == 3));
    run.settled();
    run.ok(&["qa", "menu", "palette.open"]);
    let theirs = run.ok(&["qa", "palette-type", "locked"]);
    assert!(theirs["palette"]["rows"].as_array().unwrap().iter().any(|row| row["id"].as_str().unwrap().starts_with("tab:")), "the person sees their tab: {}", theirs["palette"]);
    run.ok(&["qa", "palette-key", "escape"]);
    let agents = run.ok(&["palette", "locked"]);
    assert!(agents["items"].as_array().unwrap().iter().all(|item| !item["id"].as_str().unwrap().starts_with("tab:")), "an agent gets nothing of it: {agents}");
}

/// Failure: `recto palette --run` at Control does what its direct command needs more for (review, round 1): installing an update, which needs Full; clearing history without the checks `clear-history` makes; undo, redo and rotate taken as the owner's steps; or writing the clipboard below `copy`'s level. What has an agent's command runs as that, at the agent's level; the rest needs Full.
#[test]
fn the_palettes_run_keeps_to_each_commands_level() {
    let Some(run) = Run::start() else { return };
    let file = numbered(4);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.state.set_level(Level::Control);
    let zoomed = run.ok(&["palette", "zoom in", "--run"]);
    assert_eq!(zoomed["ran"]["id"], "view.zoom_in", "{zoomed}");
    let (update, code) = run.state.json(&["palette", "check for updates", "--run"]);
    assert_ne!(code, 0, "an update's install needs Full: {update}");
    assert_eq!(update["error"]["code"], "permission_denied", "{update}");
    let rotated = run.ok(&["palette", "rotate all pages right", "--run"]);
    assert_eq!(rotated["ran"]["id"], "rotate.right", "{rotated}");
    let edits = run.ok(&["edits"]);
    assert_eq!(edits["edited"], true, "{edits}");
    // Below Full, `discard` throws away only agents' steps: it going through says the step is the agent's, as `page rotate` makes it, and not the owner's.
    let (discarded, code) = run.state.json(&["discard"]);
    assert_eq!(code, 0, "the step is the agent's: {discarded}");
    run.state.set_level(Level::ReadOnly);
    assert!(run.ok(&["palette", "zoom"])["items"].as_array().is_some_and(|items| !items.is_empty()), "listing is reading");
    let (refused, code) = run.state.json(&["palette", "zoom in", "--run"]);
    assert_ne!(code, 0, "running is doing: {refused}");
}

/// Waits for the sheet over the window to be of this kind, and gives the state then.
fn sheet_of(run: &Run, kind: &str) -> Value {
    wait_for(run, |state| state["app"]["sheet"]["kind"] == kind)
}

/// Types a password into the sheet through `qa password`, which reads it from stdin, never the command line.
fn type_password(run: &Run, password: &str) -> Value {
    let (reply, code) = run.state.json_with_stdin(&["qa", "password"], password);
    assert_eq!((code, &reply["ok"]), (0, &json!(true)), "qa password: {reply}");
    reply["data"].clone()
}

/// Every file under a folder, and how many there were.
fn walk(root: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut waiting = vec![root.to_path_buf()];
    while let Some(folder) = waiting.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() { waiting.push(path) } else { out.push(path) }
        }
    }
    out
}

/// The files under a folder that hold these bytes, after asserting there were files to look in, so an empty or missing folder can't pass for one without the password.
fn files_holding(root: &std::path::Path, needle: &[u8]) -> Vec<PathBuf> {
    let files = walk(root);
    assert!(files.iter().any(|file| std::fs::metadata(file).is_ok_and(|meta| meta.len() > 0)), "there were no files to look in under {root:?}: {files:?}");
    files.into_iter().filter(|file| std::fs::read(file).is_ok_and(|bytes| bytes.windows(needle.len()).any(|window| window == needle))).collect()
}

/// Failure (review O1, O9): a protected PDF just says it needs a password and can't be opened; a wrong password opens it, or the tries aren't counted; the right one doesn't draw its pages or show its contents to the person; once it's open an agent gets anything decrypted from it, at any level, through `sidebar`, `where`, `tabs`, `contents`, `facts`, `search` or `text`; or the password reaches the state folder, while the app runs or after it quits.
#[test]
fn a_protected_pdf_asks_for_its_password_and_keeps_it_nowhere() {
    let Some(run) = Run::start() else { return };
    let file = TempPdf::new("locked.pdf", &common::locked_with("open sesame", 2));
    run.ok(&["qa", "open-file", file.0.to_str().unwrap()]);
    let asked = sheet_of(&run, "password");
    assert!(asked["app"]["sheet"]["informative"].as_str().unwrap().contains("locked.pdf"), "{asked}");
    type_password(&run, "not it");
    wait_for(&run, |state| state["app"]["sheet"]["informative"].as_str().is_some_and(|text| text.contains("4 tries left")));
    type_password(&run, "open sesame");
    wait_for(&run, |state| state["app"]["sheet"] == json!({}) && state["app"]["place_label"] == "1 of 2");
    run.settled();
    // The sidebar shows its one chapter, read with the password: there's something to keep from agents.
    let open = wait_for(&run, |state| state["app"]["contents"]["count"] == 1);
    assert!(open["reader"]["tiles"]["attached"].as_u64().unwrap() > 0 && open["reader"]["note"].is_null(), "its pages are drawn, and the person sees its contents: {open}");
    // What an agent gets, at every level: refused, or nothing read from the file.
    let decrypted = |value: &Value| {
        let text = value.to_string();
        text.contains(common::LOCKED_CHAPTER) || text.contains("Locked page") || text.contains("\"pages\":2") || text.contains("of 2")
    };
    let commands: [&[&str]; 7] = [&["sidebar"], &["where"], &["tabs"], &["contents"], &["facts"], &["search", "Locked"], &["text", "1"]];
    for level in [Level::Off, Level::ReadOnly, Level::Control, Level::Full] {
        run.state.set_level(level);
        for args in commands {
            let (reply, code) = run.state.json(args);
            assert!(!decrypted(&reply), "{args:?} at {level:?} gave what's decrypted: {reply}");
            if code != 0 {
                assert!([exit::PERMISSION, exit::PROTECTED].contains(&code), "{args:?} at {level:?}: {reply}");
            } else if args[0] == "where" {
                assert_eq!(reply["data"]["protected"], true, "and says why: {reply}");
            }
            if level >= Level::ReadOnly && ["contents", "facts", "search", "text"].contains(&args[0]) {
                assert_eq!((code, &reply["error"]["code"]), (exit::PROTECTED, &json!("protected")), "{args:?} at {level:?}: {reply}");
            }
        }
    }
    run.state.set_level(Level::Full);
    run.ok(&["session", "save"]);
    assert!(files_holding(&run.state.root, b"open sesame").is_empty(), "the password is on disk while it runs");
    let pid = run.pid;
    run.ok(&["stop"]);
    for _ in 0..500 {
        if !still_running(pid) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!still_running(pid), "the app didn't quit");
    assert!(files_holding(&run.state.root, b"open sesame").is_empty(), "the password is on disk after a quit");
}

/// Failure: wrong passwords are asked for without end, or Cancel leaves the sheet coming back at once; the person isn't told why it stopped.
#[test]
fn wrong_passwords_are_bounded_and_cancel_is_heard() {
    let Some(run) = Run::start() else { return };
    let file = TempPdf::new("locked.pdf", &common::locked_with("right", 1));
    run.ok(&["qa", "open-file", file.0.to_str().unwrap()]);
    sheet_of(&run, "password");
    run.ok(&["qa", "sheet-answer", "second"]);
    let cancelled = wait_for(&run, |state| state["app"]["sheet"] == json!({}));
    assert_eq!(cancelled["app"]["home"], true, "nothing opened: {cancelled}");
    run.ok(&["qa", "open-file", file.0.to_str().unwrap()]);
    for tries_left in (1..5).rev() {
        sheet_of(&run, "password");
        type_password(&run, "wrong");
        wait_for(&run, |state| state["app"]["sheet"]["informative"].as_str().is_some_and(|text| text.contains(&format!("{tries_left} tr"))));
    }
    type_password(&run, "wrong");
    let stopped = wait_for(&run, |state| state["app"]["copy_notice"]["text"].as_str().is_some_and(|text| text.contains("stopped asking")));
    assert_eq!((&stopped["app"]["sheet"], &stopped["app"]["home"]), (&json!({}), &json!(true)), "{stopped}");
}

/// Failure (review O2): with two protected files opened together, the second's ask takes the first's place, or a password typed for the file the sheet names is tried on the other, so neither opens, or the wrong one does.
#[test]
fn two_protected_files_opened_together_each_get_their_own_password() {
    let Some(run) = Run::start() else { return };
    let first = TempPdf::new("first.pdf", &common::locked_with("one", 1));
    let second = TempPdf::new("second.pdf", &common::locked_with("two", 2));
    run.ok(&["qa", "open-file", first.0.to_str().unwrap()]);
    run.ok(&["qa", "open-file", second.0.to_str().unwrap()]);
    let mut opened = Vec::new();
    for _ in 0..2 {
        let asked = sheet_of(&run, "password");
        let names = asked["app"]["sheet"]["informative"].as_str().unwrap().to_string();
        // Whichever's asked about first, its own password opens it.
        let (password, name) = if names.contains("first.pdf") { ("one", "first") } else { assert!(names.contains("second.pdf"), "{asked}"); ("two", "second") };
        assert!(!opened.contains(&name), "asked about {name} twice: {asked}");
        type_password(&run, password);
        wait_for(&run, |state| state["session"]["tabs"].as_array().is_some_and(|tabs| tabs.iter().any(|tab| tab["path"].as_str().is_some_and(|path| path.ends_with(&format!("{name}.pdf"))))));
        opened.push(name);
    }
    let both = wait_for(&run, |state| state["app"]["sheet"] == json!({}) && state["session"]["tabs"].as_array().is_some_and(|tabs| tabs.len() == 2));
    assert!(both["app"]["copy_notice"].to_string().find("didn't open").is_none(), "{both}");
}

/// Failure (review O5): passwords are kept by path, so a protected file moved while it's open loses its password, and its pages, drawn again, say it needs one, with no sheet to type it in.
#[test]
fn a_protected_file_moved_while_open_keeps_its_password() {
    let Some(run) = Run::start() else { return };
    let file = TempPdf::new("locked.pdf", &common::locked_with("open sesame", 2));
    run.ok(&["qa", "open-file", file.0.to_str().unwrap()]);
    sheet_of(&run, "password");
    type_password(&run, "open sesame");
    wait_for(&run, |state| state["app"]["sheet"] == json!({}) && state["app"]["place_label"] == "1 of 2");
    run.settled();
    run.ok(&["qa", "frontmost", "off"]);
    let moved = file.0.with_file_name("moved away.pdf");
    std::fs::rename(&file.0, &moved).unwrap();
    run.ok(&["qa", "frontmost", "on"]);
    wait_for(&run, |state| state["session"]["tabs"][0]["path"].as_str().is_some_and(|path| path.ends_with("moved away.pdf")));
    // Drawn again at another zoom, by a renderer opened on the file where it is now.
    run.ok(&["qa", "zoom", "2"]);
    run.settled();
    let after = run.report();
    assert_eq!(after["app"]["sheet"], json!({}), "it didn't need asking again: {after}");
    assert!(after["reader"]["note"].is_null() && after["reader"]["tiles"]["attached"].as_u64().unwrap() > 0, "its pages are drawn: {after}");
    std::fs::rename(&moved, &file.0).ok();
}

/// Failure (review O6): `qa password` types into the sheet for any file, one of the person's own included, rather than only a quiet run's test file in its state folder or a temp folder; or it takes the password on the command line, where other processes see it.
#[test]
fn qa_password_is_only_for_a_tests_own_file() {
    let Some(run) = Run::start() else { return };
    let outside = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("not-a-test-folder-{}", std::process::id()));
    std::fs::create_dir_all(&outside).unwrap();
    let file = outside.join("locked.pdf");
    std::fs::write(&file, common::locked_with("open sesame", 1)).unwrap();
    run.ok(&["qa", "open-file", file.to_str().unwrap()]);
    sheet_of(&run, "password");
    let (reply, code) = run.state.json_with_stdin(&["qa", "password"], "open sesame");
    assert_ne!(code, 0, "{reply}");
    assert!(reply["error"]["message"].as_str().unwrap().contains("test's own file"), "{reply}");
    assert_eq!(run.report()["session"]["tabs"].as_array().map_or(0, Vec::len), 0, "nothing opened");
    let (reply, code) = run.state.json(&["qa", "password", "open sesame"]);
    assert_eq!(code, exit::USAGE, "no password on the command line: {reply}");
    std::fs::remove_dir_all(&outside).ok();
}

/// Failure (review O8): File > Open Recent misses files opened, lists them out of order, opens another file than the item showed once the list has moved, doesn't open one, or Clear Menu leaves any.
#[test]
fn open_recent_lists_opens_and_clears() {
    let Some(run) = Run::start() else { return };
    let (a, b) = (numbered(3), outlined());
    run.ok(&["open", a.0.to_str().unwrap()]);
    run.ok(&["open", b.0.to_str().unwrap()]);
    run.settled();
    let recent_items = |state: &Value| -> Vec<(String, String)> {
        let file = state["app"]["menu_bar"].as_array().unwrap().iter().find(|menu| menu["title"] == "File").unwrap()["items"].as_array().unwrap().clone();
        file.iter().filter_map(|item| Some((item["id"].as_str()?.to_string(), item["title"].as_str()?.to_string()))).filter(|(id, _)| id.starts_with("recent.") && id != "recent.clear").collect()
    };
    let state = run.report();
    let ids: Vec<String> = state["app"]["menu_bar"].as_array().unwrap().iter().find(|menu| menu["title"] == "File").unwrap()["items"].as_array().unwrap().iter().map(|item| item["id"].as_str().unwrap().to_string()).collect();
    let at = ids.iter().position(|id| id == "file.open_recent").expect("Open Recent in File");
    assert!(ids[at + 1].starts_with("recent.") && ids[at + 2].starts_with("recent.") && ids[at + 3] == "recent.clear", "{ids:?}");
    let items = recent_items(&state);
    let name = |document: &TempPdf| document.0.file_stem().unwrap().to_str().unwrap().to_string();
    assert_eq!(items.iter().map(|(_, title)| title.clone()).collect::<Vec<_>>(), [name(&b), name(&a)], "newest first: {items:?}");
    let listed = run.ok(&["recent"]);
    assert_eq!(listed["count"], 2);
    assert_eq!(listed["files"][0]["untrusted"]["path"], b.0.to_str().unwrap(), "newest first: {listed}");
    // The item for the older file, kept from before the list moves: it opens that file, not whatever's at its old place in the list.
    let older = items[1].0.clone();
    run.ok(&["close", "--tab", "#1"]);
    run.ok(&["open", a.0.to_str().unwrap()]);
    run.ok(&["close"]);
    run.settled();
    assert_eq!(recent_items(&run.report())[0].1, name(&a), "the list moved, and only the other file is open");
    run.ok(&["qa", "menu", &older]);
    let a_name = a.0.file_name().unwrap().to_str().unwrap().to_string();
    wait_for(&run, |state| state["session"]["tabs"].as_array().is_some_and(|tabs| tabs.len() == 2 && tabs.iter().any(|tab| tab["path"].as_str().is_some_and(|path| path.ends_with(&a_name)))));
    run.ok(&["qa", "menu", "recent.clear"]);
    let cleared = wait_for(&run, |state| state["session"]["recent"].is_null() || state["session"]["recent"].as_array().is_some_and(Vec::is_empty));
    assert!(recent_items(&cleared).is_empty(), "{cleared}");
}

/// Failure: a file deleted, moved or replaced while it's open isn't noticed when the person comes back to the app, so they read a file that's gone or an old version as the new one, with nothing said.
#[test]
fn a_file_changed_while_open_is_said_when_the_app_comes_back() {
    let Some(run) = Run::start() else { return };
    let file = numbered(3);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "frontmost", "off"]);
    let moved = file.0.with_file_name("moved away.pdf");
    std::fs::rename(&file.0, &moved).unwrap();
    run.ok(&["qa", "frontmost", "on"]);
    let followed = wait_for(&run, |state| state["session"]["tabs"][0]["path"].as_str().is_some_and(|path| path.ends_with("moved away.pdf")));
    assert!(followed["session"]["tabs"][0]["notice"]["moved"].is_object(), "and says so: {followed}");
    run.ok(&["qa", "frontmost", "off"]);
    std::fs::write(&moved, recto_testpdf::pages(&[Page::new(text_lines(&["Another version"]))])).unwrap();
    run.ok(&["qa", "frontmost", "on"]);
    let changed = wait_for(&run, |state| state["app"]["note"].as_str().is_some_and(|note| note.contains("changed on disk")));
    assert_eq!(changed["session"]["tabs"][0]["page_count"], 1, "the new version is what's shown: {changed}");
    run.ok(&["qa", "frontmost", "off"]);
    std::fs::remove_file(&moved).unwrap();
    run.ok(&["qa", "frontmost", "on"]);
    wait_for(&run, |state| state["app"]["note"].as_str().is_some_and(|note| note.contains("can't be found")));
    std::fs::rename(&moved, &file.0).ok();
}

fn named_document(name: &str, count: usize, word: &str) -> TempPdf {
    let list: Vec<Page> = (0..count).map(|index| Page::new(text_lines(&[&format!("{name} page {index}"), &format!("Here the word {word} shows")]))).collect();
    TempPdf::new(&format!("{name}.pdf"), &pages(&list))
}

/// Failure: Find's field, list and steps don't do what `recto find` does, or the list grows over the tabs and contents (owner, 2026-11-07: "max height within scroll so it doesn't push everything of screen"), or Find takes room or keeps its hits once it's closed.
#[test]
fn find_in_the_sidebar_lists_steps_and_keeps_to_its_room() {
    let Some(run) = Run::start() else { return };
    let file = numbered(200);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let closed = run.report();
    assert_eq!((closed["app"]["find"]["open"].as_bool(), closed["app"]["find"]["shown"].as_bool()), (Some(false), Some(false)), "nothing at rest: {}", closed["app"]["find"]);
    let opened = run.ok(&["qa", "menu", "find.open"]);
    let opened = wait_for(&run, |state| state["app"]["find"]["open"] == true).get("app").cloned().unwrap_or(opened);
    assert_eq!(opened["focus_view"], "find", "⌘F gives the field the keyboard: {}", opened["find"]);
    run.ok(&["qa", "find-type", "words"]);
    run.settled();
    let found = run.report();
    let find = &found["app"]["find"];
    assert_eq!((find["status"].as_str(), find["rows"].as_u64()), (Some("200 matches on 200 pages"), Some(400)), "{find}");
    let room = found["app"]["window"]["height"].as_f64().unwrap();
    assert!(find["list_height"].as_f64().unwrap() <= room * 0.4 && find["list_height"].as_f64().unwrap() > 0.0, "{find}");
    assert_eq!(found["app"]["contents"]["panel_button_on_screen"], true, "the tabs and the bottom bar stay: {}", found["app"]["contents"]);
    let stepped = run.ok(&["qa", "find-key", "return"]);
    run.settled();
    let stepped = run.report().get("app").cloned().unwrap_or(stepped);
    assert_eq!((stepped["find"]["current_row"].as_u64(), stepped["place"].as_str()), (Some(1), Some("1 of 200")), "{}", stepped["find"]);
    assert!(stepped["pages"]["highlight_rects"].as_u64().unwrap() > 0, "the hit is selected on the page");
    run.ok(&["qa", "find-click", "5"]);
    run.settled();
    let clicked = run.report();
    assert_eq!((clicked["app"]["find"]["current_row"].as_u64(), clicked["app"]["place"].as_str()), (Some(5), Some("3 of 200")), "{}", clicked["app"]["find"]);
    run.ok(&["qa", "menu", "find.previous"]);
    run.settled();
    assert_eq!(run.report()["app"]["find"]["current_row"], 3, "⇧⌘G");
    let agent = run.ok(&["find", "--next"]);
    assert_eq!(agent["current"]["page"], 3, "an agent's step goes on from the reader's: {agent}");
    run.ok(&["qa", "find-key", "escape"]);
    run.settled();
    let gone = run.report();
    assert_eq!((gone["app"]["find"]["open"].as_bool(), gone["app"]["find"]["shown"].as_bool()), (Some(false), Some(false)), "{}", gone["app"]["find"]);
    let idle = run.ok(&["find"]);
    assert_eq!((idle["state"].as_str(), idle["hits_listed"].as_u64()), (Some("idle"), Some(0)), "its hits went: {idle}");
    assert_eq!(gone["session"]["find"]["draft"], "words", "what was typed stays for next time");
}

/// Failure: in a comparison Find looks in a pane that isn't the one being read, or stays with the old one when the reader moves (PLAN, comparison follow-ups).
#[test]
fn find_goes_by_the_pane_being_read() {
    let Some(run) = Run::start() else { return };
    let (left, right) = (named_document("left", 6, "apple"), named_document("right", 6, "quince"));
    run.ok(&["open", left.0.to_str().unwrap()]);
    run.ok(&["open", right.0.to_str().unwrap()]);
    run.ok(&["compare", "--tabs", "#1,#2"]);
    run.settled();
    run.ok(&["qa", "pane-focus", "1"]);
    run.ok(&["qa", "menu", "find.open"]);
    run.ok(&["qa", "find-type", "quince"]);
    run.settled();
    assert_eq!(run.report()["app"]["find"]["status"], "6 matches on 6 pages");
    run.ok(&["qa", "pane-focus", "0"]);
    run.settled();
    assert_eq!(run.report()["app"]["find"]["status"], "Not found", "found again in the pane now read");
    run.ok(&["qa", "find-type", "apple"]);
    run.settled();
    run.ok(&["qa", "find-key", "return"]);
    run.settled();
    let state = run.report();
    assert_eq!(state["app"]["find"]["current_row"], 1, "{}", state["app"]["find"]);
}

/// Failure: edit mode and a view's rotation are in the state record and nothing shows them (PLAN phase 3: they "draw what the state record already carries"), or Done doesn't leave edit mode.
#[test]
fn edit_mode_and_view_rotation_draw_what_the_record_carries() {
    let Some(run) = Run::start() else { return };
    let file = numbered(3);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "view-rotation", "1"]);
    run.settled();
    let turned = run.report();
    let turns = turned["app"]["pages"]["page_turns"].as_array().unwrap().clone();
    assert!(!turns.is_empty() && turns.iter().all(|turn| turn == 1), "every page drawn a quarter turn: {turns:?}");
    assert_eq!(run.ok(&["edits"])["edited"], false, "and the file isn't edited by it");
    run.ok(&["qa", "edit-mode", "on"]);
    run.settled();
    let panel = run.report()["app"]["panel"].clone();
    assert_eq!(panel["open"], true, "the panel is open on edit mode's tools: {panel}");
    assert_eq!(panel["items"][0]["id"], "edit_mode.done", "{panel}");
    run.ok(&["qa", "panel", "edit_mode.done"]);
    run.settled();
    let done = run.report();
    assert_eq!((done["app"]["panel"]["open"].as_bool(), done["session"]["edit_mode"].as_bool()), (Some(false), Some(false)), "{}", done["app"]["panel"]);
    run.ok(&["qa", "view-rotation", "0"]);
    run.settled();
    assert!(run.report()["app"]["pages"]["page_turns"].as_array().unwrap().iter().all(|turn| turn == 0));
}

/// Failure: what Rust last heard of the field is put back into it while it's being typed in, so a late echo of "ab" takes away the "c" just typed (review, round 1). Here the session's word on the field changes under the editing field, as an echo would, and the field keeps what's typed; once it's left, it shows Rust's.
#[test]
fn find_never_types_over_what_is_being_typed() {
    let Some(run) = Run::start() else { return };
    let file = numbered(300);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "find", "open"]);
    run.ok(&["qa", "focus", "find"]);
    for typed in ["w", "wo", "wor", "word"] {
        let state = run.ok(&["qa", "find-type", typed]);
        assert_eq!(state["find"]["untrusted"]["field"], typed, "{}", state["find"]);
    }
    run.ok(&["qa", "find-draft", "an echo"]);
    run.settled();
    let editing = run.report();
    assert_eq!(editing["app"]["find"]["field_has_focus"], true);
    assert_eq!(editing["app"]["find"]["untrusted"]["field"], "word", "never typed over while it's edited: {}", editing["app"]["find"]);
    run.ok(&["qa", "focus", "page"]);
    let left = wait_for(&run, |state| state["app"]["find"]["untrusted"]["field"] == "an echo");
    assert_eq!(left["app"]["find"]["field_has_focus"], false);
}

/// Failure: the field takes the keyboard just before the session's draft reaches it, and then shows the old text for as long as it's focused, though nothing's been typed in it (the restore test after the merge with main: field "" with the draft "typed but not run"). Until typing starts, the field shows what the session has.
#[test]
fn find_shows_the_sessions_draft_until_typing_starts() {
    let Some(run) = Run::start() else { return };
    let file = numbered(3);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "find", "open"]);
    run.ok(&["qa", "focus", "find"]);
    run.ok(&["qa", "find-draft", "set while focused"]);
    let shown = wait_for(&run, |state| state["app"]["find"]["untrusted"]["field"] == "set while focused");
    assert_eq!(shown["app"]["find"]["field_has_focus"], true, "still focused: {}", shown["app"]["find"]);
    assert_eq!(shown["session"]["find"]["query"], "", "and not found, as it was only a draft: {}", shown["session"]["find"]);
}

/// Failure: the app draws its pages with a worker outside the sandbox, or the tile path breaks once the worker has dropped its Mach rights, and nothing an agent can read says so.
/// The built app's helper is sandboxed, so every live worker must say its walls are up, and the tiles on screen came as IOSurfaces over the link, the only way one arrives.
#[test]
fn the_app_draws_with_walled_workers_over_the_surface_link() {
    let Some(run) = Run::start() else { return };
    let file = numbered(3);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let state = run.report();
    let workers = state["memory"]["workers"].as_u64().unwrap();
    assert!(workers >= 1, "no worker drew the page: {}", state["memory"]);
    assert_eq!(state["memory"]["workers_walled"].as_u64(), Some(workers), "a live worker isn't walled: {}", state["memory"]);
    assert!(state["reader"]["tiles"]["attached"].as_u64().unwrap() > 0, "no surface tile arrived: {}", state["reader"]["tiles"]);
}

/// A place in the middle of one of `linked_document`'s links on page 1, as `qa pointer` takes it.
fn on_link(index: usize) -> String {
    let rect = recto_testpdf::LINKS_ON_PAGE_ONE[index].0;
    format!("1:{},{}", (rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0)
}

/// The pointer over a link, waiting for its page's links to be read, as a person's next move would see them.
fn hover(run: &Run, place: &str) -> Value {
    run.ok(&["qa", "pointer", place]);
    run.settled();
    wait_for(run, |state| {
        run.ok(&["qa", "pointer", place]);
        state["app"]["pages"]["cursor"] == "link"
    })
}

/// Failure: a link in the app doesn't show where it goes, or a click goes somewhere else or nowhere; Back and Forward, from the Go menu or a swipe, don't retrace it; a web address opens without the person being asked, or doesn't show its real target first; a script runs; or the page number in the corner and the go-to field don't know what the book calls its pages.
#[test]
fn links_back_forward_and_page_labels_in_the_app() {
    let Some(run) = Run::start() else { return };
    let file = TempPdf::new("linked.pdf", &recto_testpdf::linked_document());
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let rest = run.report();
    assert_eq!(rest["app"]["place_label"], "i (1 of 12)", "the book's own number, and its place: {rest}");
    let over = hover(&run, &on_link(0));
    assert_eq!(over["app"]["links"]["hint"], "Go to page 5 (page 9)", "{over}");
    assert_eq!(over["app"]["links"]["hint_in_lower_left"], true);
    run.ok(&["qa", "mouse-down", &format!("{}:1", on_link(0))]);
    run.ok(&["qa", "mouse-up"]);
    let went = wait_for(&run, |state| state["app"]["place_label"] == "5 (9 of 12)");
    let go = |state: &Value, id: &str| state["app"]["menu_bar"].as_array().unwrap().iter().flat_map(|menu| menu["items"].as_array().unwrap()).find(|item| item["id"] == id).map(|item| item["enabled"].clone()).unwrap();
    assert_eq!((go(&went, "go.back"), go(&went, "go.forward")), (json!(true), json!(false)), "{went}");
    run.ok(&["qa", "menu", "go.back"]);
    let back = wait_for(&run, |state| state["app"]["place_label"] == "i (1 of 12)");
    assert_eq!(go(&back, "go.forward"), json!(true));
    run.ok(&["qa", "swipe", "-1"]);
    wait_for(&run, |state| state["app"]["place_label"] == "5 (9 of 12)");
    run.ok(&["qa", "swipe", "1"]);
    wait_for(&run, |state| state["app"]["place_label"] == "i (1 of 12)");
    // A web address: asked about first, with the whole address, and opened only on Open. A quiet run opens nothing and notes it.
    let web = hover(&run, &on_link(2));
    assert_eq!(web["app"]["links"]["hint"], "Web link: https://example.com/report?id=7");
    run.ok(&["qa", "mouse-down", &format!("{}:1", on_link(2))]);
    let asked = run.ok(&["qa", "mouse-up"]);
    let asked = if asked["sheet"]["kind"] == "link_prompt" { run.report() } else { wait_for(&run, |state| state["app"]["sheet"]["kind"] == "link_prompt") };
    assert!(asked["app"]["sheet"]["informative"].as_str().unwrap().contains("https://example.com/report?id=7"), "{asked}");
    assert_eq!(asked["app"]["links"]["address_opened"], Value::Null, "nothing opens before the answer");
    let cancelled = run.ok(&["qa", "sheet-answer", "second"]);
    assert_eq!(cancelled["links"]["address_opened"], Value::Null, "Cancel opens nothing: {cancelled}");
    run.ok(&["qa", "mouse-down", &format!("{}:1", on_link(2))]);
    run.ok(&["qa", "mouse-up"]);
    wait_for(&run, |state| state["app"]["sheet"]["kind"] == "link_prompt");
    let opened = run.ok(&["qa", "sheet-answer", "first"]);
    assert_eq!(opened["links"]["address_opened"], "https://example.com/report?id=7", "{opened}");
    // A script: never run, and said so.
    hover(&run, &on_link(3));
    run.ok(&["qa", "mouse-down", &format!("{}:1", on_link(3))]);
    run.ok(&["qa", "mouse-up"]);
    let refused = wait_for(&run, |state| state["app"]["copy_notice"]["text"].as_str().is_some_and(|text| text.contains("script")));
    assert_eq!((&refused["app"]["sheet"], &refused["app"]["place_label"]), (&json!({}), &json!("i (1 of 12)")), "{refused}");
    // The go-to field, by label and by number, and something that names no page.
    let field = run.ok(&["qa", "button", "page"]);
    assert_eq!(field["links"]["page_field"], "i", "it starts with what the book calls this page: {field}");
    run.ok(&["qa", "page-escape"]);
    run.ok(&["qa", "page-type", "A-2"]);
    wait_for(&run, |state| state["app"]["place_label"] == "A-2 (12 of 12)");
    run.ok(&["qa", "page-type", "iii"]);
    wait_for(&run, |state| state["app"]["place_label"] == "iii (3 of 12)");
    run.ok(&["qa", "page-type", "no such page"]);
    wait_for(&run, |state| state["app"]["copy_notice"]["text"].as_str().is_some_and(|text| text.contains("No page is called")));
    // An agent sees the same, and goes back the same way.
    let place = run.ok(&["where"]);
    assert_eq!((&place["page"], &place["untrusted"]["label"], &place["can_go_back"]), (&json!(3), &json!("iii"), &json!(true)), "{place}");
    run.ok(&["back"]);
    wait_for(&run, |state| state["app"]["place_label"] == "A-2 (12 of 12)");
}

fn found_in(bytes: &[u8], needle: &str) -> bool {
    bytes.windows(needle.len()).any(|window| window == needle.as_bytes())
}

/// Three pages, the second marked to be deleted.
fn three_marked() -> TempPdf {
    TempPdf::new("three.pdf", &pages(&[Page::new(text_lines(&["Keep page one"])), Page::new(text_lines(&["DELETEDPAGEMARK"])), Page::new(text_lines(&["Keep page three"]))]))
}

/// A two-page document with a person's signature, or certified, or with usage rights alone.
fn signed_two(kind: &str) -> TempPdf {
    let (form, perms) = match kind {
        "certified" => ("/AcroForm << /Fields [5 0 R] /SigFlags 3 >>", "/Perms << /DocMDP 6 0 R >>"),
        "usage_rights" => ("", "/Perms << /UR3 6 0 R >>"),
        _ => ("/AcroForm << /Fields [5 0 R] /SigFlags 3 >>", ""),
    };
    TempPdf::new(&format!("{kind}.pdf"), &build(&[
        Object::Dict(format!("<< /Type /Catalog /Pages 2 0 R {form} {perms} >>")),
        Object::Dict("<< /Type /Pages /Kids [3 0 R 7 0 R] /Count 2 >>".into()),
        Object::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".into()),
        Object::Stream { dict: String::new(), data: text_lines(&["A signed contract, page one"]) },
        Object::Dict("<< /FT /Sig /T (Tenant) /V 6 0 R >>".into()),
        Object::Dict("<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 5 10 5] /Contents <00> >>".into()),
        Object::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 8 0 R >>".into()),
        Object::Stream { dict: String::new(), data: text_lines(&["DELETEDPAGEMARK"]) },
    ]))
}

/// Failure: a delete from the window isn't saved, is appended so the page's words stay in the file, or can't be undone past the save, or undoing past it needs a copy of the whole file. Through the window's own Save: the page is gone from the file, the history keeps a copy of that page alone, out of Spotlight's reach, and undo past the save brings it back and draws it.
#[test]
fn a_delete_saves_by_rewriting_and_undo_goes_past_it() {
    let Some(run) = Run::start() else { return };
    let file = three_marked();
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "owner-edit", "delete:2"]);
    let edited = wait_for(&run, |state| state["app"]["save_button"]["visible"] == true && state["reader"]["pages"] == 2);
    assert_eq!(edited["app"]["save_button"]["enabled"], true, "a delete on an unsigned file saves: {}", edited["app"]["save_button"]);
    run.ok(&["qa", "button", "save"]);
    wait_for(&run, |state| state["app"]["document_edited"] == false && state["app"]["save_button"]["visible"] == false);
    let bytes = std::fs::read(&file.0).unwrap();
    assert!(!found_in(&bytes, "DELETEDPAGEMARK") && found_in(&bytes, "Keep page three"), "the page's words are gone from the file");
    let journals = run.state.root.join("session").join("journals");
    let kept: Vec<PathBuf> = std::fs::read_dir(&journals).unwrap().flatten().map(|entry| entry.path()).filter(|path| path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("kept-"))).collect();
    assert_eq!(kept.len(), 1, "one copy kept for the history: {kept:?}");
    assert!(kept[0].extension().is_some_and(|extension| extension == "keptpages"), "not a .pdf, so Spotlight has no importer for it");
    let kept_bytes = std::fs::read(&kept[0]).unwrap();
    assert!(found_in(&kept_bytes, "DELETEDPAGEMARK") && !found_in(&kept_bytes, "Keep page one"), "it holds the deleted page alone");
    // Undo past the save: the page comes back, from the copy, and draws.
    run.ok(&["qa", "owner-edit", "undo"]);
    let back = wait_for(&run, |state| state["reader"]["pages"] == 3 && state["app"]["document_edited"] == true);
    assert_eq!(back["app"]["save_button"]["visible"], true);
    run.ok(&["goto", "2"]);
    run.settled();
    let drawn = run.report();
    assert!(drawn["reader"]["note"].is_null(), "page 2 draws from the kept copy: {}", drawn["reader"]["note"]);
    assert_eq!(run.ok(&["text", "2"])["untrusted"]["text"].as_str().map(|text| text.contains("DELETEDPAGEMARK")), Some(true));
}

/// Failure (SPEC decision 27): a delete on a signed file can be saved over it, which would break the signature, or leaves no way to keep the change. Save is off, and its icon says why; ⌘S is greyed out; Save As's sheet says the copy won't be signed; and the copy has no signature, while the original is byte for byte as it was.
#[test]
fn a_signed_files_delete_turns_save_off_and_save_as_is_unsigned() {
    let Some(run) = Run::start() else { return };
    for kind in ["signed", "certified"] {
        let file = signed_two(kind);
        let original = std::fs::read(&file.0).unwrap();
        run.ok(&["open", file.0.to_str().unwrap()]);
        run.settled();
        // A certification forbids a turn too; a person's signature only what has to be written anew.
        run.ok(&["qa", "owner-edit", if kind == "certified" { "rotate:1" } else { "delete:2" }]);
        let off = wait_for(&run, |state| state["app"]["save_button"]["visible"] == true && state["app"]["save_button"]["enabled"] == false);
        assert!(off["app"]["save_button"]["tooltip"].as_str().unwrap().contains("Save As makes an unsigned copy"), "{kind}: {}", off["app"]["save_button"]);
        assert_eq!(menu_item(&off, "file.save")["enabled"], false, "{kind}: ⌘S is off too");
        let (refused, _) = run.state.json(&["qa", "button", "save"]);
        assert_eq!(refused["ok"], false, "{kind}: a click on an icon that's off does nothing: {refused}");
        run.ok(&["qa", "menu", "file.save_as"]);
        let sheet = wait_for(&run, |state| state["app"]["sheet"]["kind"] == "save_panel");
        assert_eq!(sheet["app"]["sheet"]["message"], "The new copy won't be signed. The original keeps its signature.", "{kind}");
        run.ok(&["qa", "sheet-answer", "cancel"]);
        wait_for(&run, |state| state["app"]["sheet"].as_object().is_none_or(|sheet| sheet.is_empty()));
        let copy = file.0.with_file_name(format!("{kind} unsigned copy.pdf"));
        run.ok(&["qa", "save-as", copy.to_str().unwrap()]);
        let saved = wait_for(&run, |state| state["app"]["document_edited"] == false && !state["app"]["copy_notice"].is_null());
        assert!(saved["app"]["copy_notice"]["text"].as_str().is_some_and(|note| note.contains("The new copy won't be signed")), "{kind}: {}", saved["app"]["copy_notice"]);
        let written = std::fs::read(&copy).unwrap();
        assert!(!found_in(&written, "/ByteRange") && !found_in(&written, "/Perms"), "{kind}: the copy has no signature");
        assert_eq!(std::fs::read(&file.0).unwrap(), original, "{kind}: the original keeps its signature");
        run.ok(&["close"]);
        let _ = std::fs::remove_file(copy);
    }
}

/// Failure: a delete on a file whose only signature is usage rights, as IRS forms have, is treated as a person's signature and Save turned off, or is saved with rights a reader then calls broken. Save works, rewriting the file in place with them removed, and says so quietly.
#[test]
fn usage_rights_alone_save_in_place_with_a_quiet_note() {
    let Some(run) = Run::start() else { return };
    let file = signed_two("usage_rights");
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "owner-edit", "delete:2"]);
    let edited = wait_for(&run, |state| state["app"]["save_button"]["visible"] == true);
    assert_eq!(edited["app"]["save_button"]["enabled"], true, "usage rights aren't a person's signature: {}", edited["app"]["save_button"]);
    run.ok(&["qa", "button", "save"]);
    let saved = wait_for(&run, |state| state["app"]["document_edited"] == false && !state["app"]["copy_notice"].is_null());
    assert!(saved["app"]["copy_notice"]["text"].as_str().is_some_and(|note| note.contains("usage rights")), "{}", saved["app"]["copy_notice"]);
    let bytes = std::fs::read(&file.0).unwrap();
    assert!(!found_in(&bytes, "/UR3") && !found_in(&bytes, "DELETEDPAGEMARK"));
    // A save's note is about that document: on another tab it goes, rather than read as about that one (found in the stage 2 shots).
    let other = three_marked();
    run.ok(&["open", other.0.to_str().unwrap()]);
    let elsewhere = wait_for(&run, |state| state["app"]["document_name"] != saved["app"]["document_name"] || state["reader"]["pages"] == 3);
    run.settled();
    assert!(run.report()["app"]["copy_notice"].is_null(), "{}", elsewhere["app"]["copy_notice"]);
}

/// A page with a line of text and the 4 x 4 red JPEG of `export_svg.rs`, over 100..140 across and 252..292 down, shown from the top left.
fn snapshot_pdf() -> (TempPdf, Vec<u8>) {
    const JPEG_HEX: &str = "ffd8ffe000104a46494600010100000100010000ffdb0043000302020302020303030304030304050805050404050a070706080c0a0c0c0b0a0b0b0d0e12100d0e110e0b0b1016101113141515150c0f171816141812141514ffdb00430103040405040509050509140d0b0d1414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414ffc00011080004000403012200021101031101ffc4001f0000010501010101010100000000000000000102030405060708090a0bffc400b5100002010303020403050504040000017d01020300041105122131410613516107227114328191a1082342b1c11552d1f02433627282090a161718191a25262728292a3435363738393a434445464748494a535455565758595a636465666768696a737475767778797a838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae1e2e3e4e5e6e7e8e9eaf1f2f3f4f5f6f7f8f9faffc4001f0100030101010101010101010000000000000102030405060708090a0bffc400b51100020102040403040705040400010277000102031104052131061241510761711322328108144291a1b1c109233352f0156272d10a162434e125f11718191a262728292a35363738393a434445464748494a535455565758595a636465666768696a737475767778797a82838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae2e3e4e5e6e7e8e9eaf2f3f4f5f6f7f8f9faffda000c03010002110311003f00f0aa28a2be68fedd3fffd9";
    let jpeg: Vec<u8> = (0..JPEG_HEX.len()).step_by(2).map(|at| u8::from_str_radix(&JPEG_HEX[at..at + 2], 16).unwrap()).collect();
    let image = recto_testpdf::first_extra_object(1);
    let content = [text_lines(&["Words to take in a box"]), b"q 40 0 0 40 100 500 cm /Im Do Q\n".to_vec()].concat();
    let dict = "/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode".to_string();
    let bytes = recto_testpdf::pages_with(&[Page::new(content)], &format!("/XObject << /Im {image} 0 R >>"), vec![Object::Stream { dict, data: jpeg.clone() }]);
    (TempPdf::new("snapshot.pdf", &bytes), jpeg)
}

/// The types the app put on the clipboard, in order: the system adds older names for each, and a TIFF it can make from an image, which aren't the app's.
fn clipboard_items(run: &Run) -> Vec<Value> {
    run.ok(&["qa", "copied"])["types"].as_array().unwrap().iter().filter(|item| item["type"].as_str().is_some_and(|name| name.starts_with("public.") && name != "public.tiff")).cloned().collect()
}

fn clipboard_types(run: &Run) -> Vec<String> {
    clipboard_items(run).iter().map(|item| item["type"].as_str().unwrap().to_string()).collect()
}

/// Failure: with the snapshot tool on, a drag still selects text, or the box isn't drawn while dragging, or the clipboard doesn't get PNG, text, SVG and JPG at once (SPEC section 6), or a click on a JPEG doesn't give its own bytes. Through the menu item, and the presses a real mouse makes, in window coordinates.
#[test]
fn the_snapshot_tool_takes_a_box_and_an_image_to_the_clipboard() {
    use sha2::Digest;
    let Some(run) = Run::start() else { return };
    let (file, jpeg) = snapshot_pdf();
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "menu", "tools.snapshot"]);
    let on = run.report();
    assert_eq!(on["snapshot"]["on"], true, "{on}");
    assert_eq!(menu_item(&on, "tools.snapshot")["checked"], true, "the menu says so");
    run.ok(&["qa", "mouse-down", "1:60,50"]);
    run.ok(&["qa", "mouse-drag", "1:400,300"]);
    let drawing = run.report();
    let shown = &drawing["app"]["pages"]["snapshot_box"];
    assert_eq!((shown["page"].as_u64(), shown["width"].as_f64(), shown["height"].as_f64()), (Some(0), Some(340.0), Some(250.0)), "{drawing}");
    assert_eq!(drawing["reader"]["selection"]["selected"], false, "no text selected by the tool's drag");
    run.ok(&["qa", "mouse-up"]);
    run.settled();
    let after = run.report();
    assert!(after["app"]["pages"]["snapshot_box"].is_null(), "the box goes on release: {after}");
    // The SVG is promised, and made only when a paste asks for it: seen on the first look, as a look in this process makes the system resolve it.
    let first = clipboard_items(&run);
    assert_eq!(first.iter().map(|item| item["type"].as_str().unwrap()).collect::<Vec<_>>(), ["public.png", "public.utf8-plain-text", "public.svg-image", "public.jpeg"]);
    assert_eq!(first[2]["promised"], true, "{first:?}");
    assert_eq!(copied(&run), "Words to take in a box");
    let pasted = run.ok(&["qa", "paste", "public.svg-image"]);
    let made = pasted["types"].as_array().unwrap().iter().find(|item| item["type"] == "public.svg-image").unwrap().clone();
    assert!(made["bytes"].as_u64().unwrap() > 100, "made on the paste: {pasted}");
    // A click on the image: its own JPEG, then a PNG for what can't take one.
    run.ok(&["qa", "mouse-down", "1:120,270"]);
    run.ok(&["qa", "mouse-up"]);
    run.settled();
    let items = Value::Array(clipboard_items(&run));
    assert_eq!(items[0]["type"], "public.jpeg", "{items}");
    assert_eq!(items[0]["sha256"].as_str().unwrap(), sha2::Sha256::digest(&jpeg).iter().map(|byte| format!("{byte:02x}")).collect::<String>(), "its own bytes");
    assert_eq!(items[1]["type"], "public.png");
    // A click off any image says so, and leaves the clipboard as it was.
    run.ok(&["qa", "mouse-down", "1:400,600"]);
    run.ok(&["qa", "mouse-up"]);
    run.settled();
    let told = wait_for(&run, |state| state["app"]["copy_notice"]["text"].as_str().is_some_and(|text| text.contains("no image")));
    assert!(told["app"]["copy_notice"]["button"].is_null(), "{told}");
    assert_eq!(clipboard_types(&run)[0], "public.jpeg");
    // Off again, a drag selects text as before.
    run.ok(&["qa", "menu", "tools.snapshot"]);
    assert_eq!(run.report()["snapshot"]["on"], false);
}

/// Failure: a snapshot of a file that asks not to be copied gets round it, or is refused with no "Copy anyway", though the owner decided on 2026-11-08 it gets the same notice as text. The button takes the same box again.
#[test]
fn a_snapshot_of_a_copy_restricted_file_offers_copy_anyway() {
    let Some(run) = Run::start() else { return };
    let file = restricted_pdf();
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "menu", "tools.snapshot"]);
    run.ok(&["qa", "mouse-down", "1:60,50"]);
    run.ok(&["qa", "mouse-drag", "1:400,120"]);
    run.ok(&["qa", "mouse-up"]);
    run.settled();
    let asked = wait_for(&run, |state| state["app"]["copy_notice"].is_object());
    assert!(asked["app"]["copy_notice"]["text"].as_str().unwrap().contains("permissions"), "{asked}");
    assert_eq!(asked["app"]["copy_notice"]["button"], "Copy anyway");
    assert!(clipboard_types(&run).is_empty(), "nothing on the clipboard yet");
    run.ok(&["qa", "notice-button"]);
    run.settled();
    assert_eq!(clipboard_types(&run)[0], "public.png");
    assert_eq!(copied(&run), "Words that ask not to be copied");
}

/// How many 8 px blocks of two pictures of one size differ in mean by more than 24 of 255, of how many: what a print is held to against its source, as text edges move a little when a page's picture is drawn again and nothing missing or wrong passes.
fn blocks_differing(one: &[u8], two: &[u8], width: usize, height: usize) -> (usize, usize) {
    let (mut bad, mut all) = (0, 0);
    for top in (0..height.saturating_sub(8)).step_by(8) {
        for left in (0..width.saturating_sub(8)).step_by(8) {
            let sum = |pixels: &[u8]| -> i64 { (top..top + 8).map(|y| pixels[(y * width + left) * 3..(y * width + left + 8) * 3].iter().map(|&value| i64::from(value)).sum::<i64>()).sum() };
            all += 1;
            bad += usize::from((sum(one) - sum(two)).abs() > 24 * 192);
        }
    }
    (bad, all)
}

/// Failure: the app's print isn't the document's pages: drawn from the screen's tiles, with dark paper in it, missing pages or out of order, or the UI thread waits on it. Through File > Print's own path to a PDF, page by page against the source.
#[test]
fn a_print_from_the_app_is_the_documents_pages_whatever_the_paper() {
    use recto_svg::verify::{Embedded, read_png};
    let Some(run) = Run::start() else { return };
    let page = |n: usize, colour: &str| Page::new([text_lines(&[&format!("Page {n} printed")]), format!("{colour} rg {} 300 120 80 re f\n", 60 + n * 100).into_bytes()].concat());
    let file = TempPdf::new("print-me.pdf", &pages(&[page(1, "1 0 0"), page(2, "0 0.6 0"), page(3, "0 0 1")]));
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["view", "--paper", "dark"]);
    run.settled();
    assert_eq!(menu_item(&run.report(), "file.print")["enabled"], true);
    let out = file.0.with_file_name(format!("printed-{}.pdf", std::process::id()));
    run.ok(&["qa", "print-to-pdf", &format!("{}|2|3", out.display())]);
    run.settled();
    let state = run.report();
    assert_eq!(state["printing"]["busy"], false, "{state}");
    assert!(state["hangs"]["over_1s"].as_u64() == Some(0), "the UI thread never waited: {state}");
    let local = State::at(Level::Full);
    let facts = local.json(&["local", out.to_str().unwrap(), "facts", "--json"]).0;
    assert_eq!(facts["data"]["pages"], 2, "{facts}");
    let draw = |pdf: &std::path::Path, page: u32| {
        let png = pdf.with_file_name(format!("drawn-{}-{page}.png", std::process::id()));
        let (reply, code) = local.json(&["local", pdf.to_str().unwrap(), "render", &page.to_string(), "--scale", "1", "--out", png.to_str().unwrap()]);
        assert_eq!(code, 0, "{reply}");
        let Embedded::Png { width, height, pixels, .. } = read_png(&std::fs::read(&png).unwrap()) else { panic!() };
        std::fs::remove_file(&png).unwrap();
        (width as usize, height as usize, pixels)
    };
    for (printed, source) in [(1, 2), (2, 3)] {
        let (width, height, print) = draw(&out, printed);
        let (source_width, source_height, original) = draw(&file.0, source);
        assert_eq!((width, height), (source_width, source_height));
        let (bad, all) = blocks_differing(&print, &original, width, height);
        // Up to 5% of blocks may differ: the print is a 300 dpi picture drawn back at scale 1, and its text edges come out a little differently on
        // another machine (CI read 90 of 7448, 1.2%, where 0.5% was allowed). Dark paper or another page differs in most blocks, so this still tells them apart.
        assert!(bad * 20 < all, "printed page {printed} against source page {source}: {bad} of {all} blocks differ, as dark paper or the wrong page would");
    }
    std::fs::remove_file(&out).unwrap();
}

/// Failure: a print to a PDF of a file that asks not to be copied gets round it, though the owner decided on 2026-11-08 that print to PDF gets the same step as Copy; or it's refused with no "Print anyway"; or the button prints nothing. Through File > Print's own path to a file, as the panel's Save as PDF takes it.
#[test]
fn a_print_to_a_file_of_a_copy_restricted_pdf_offers_print_anyway() {
    let Some(run) = Run::start() else { return };
    let file = restricted_pdf();
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let out = file.0.with_file_name(format!("restricted-printed-{}.pdf", std::process::id()));
    run.ok(&["qa", "print-to-pdf", out.to_str().unwrap()]);
    run.settled();
    let asked = wait_for(&run, |state| state["app"]["copy_notice"]["button"] == "Print anyway");
    assert!(asked["app"]["copy_notice"]["text"].as_str().unwrap().contains("permissions"), "{asked}");
    assert!(!out.exists(), "nothing printed yet");
    run.ok(&["qa", "notice-button"]);
    run.settled();
    let done = wait_for(&run, |state| state["printing"]["busy"] == false);
    assert!(out.exists(), "printed once asked: {done}");
    std::fs::remove_file(&out).unwrap();
}

/// Failure: a long print can't be stopped, or Stop leaves its file in the temp folder, or the print goes on to the system anyway. The progress line has Stop beside it, through the button itself.
#[test]
fn a_print_being_prepared_stops_with_its_stop_button() {
    let Some(run) = Run::start() else { return };
    let list: Vec<Page> = (0..120).map(|n| Page::new(text_lines(&[&format!("Page {n} of a long print")]))).collect();
    let file = TempPdf::new("long-print.pdf", &pages(&list));
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let out = file.0.with_file_name(format!("stopped-{}.pdf", std::process::id()));
    run.ok(&["qa", "print-to-pdf", out.to_str().unwrap()]);
    let preparing = wait_for(&run, |state| state["app"]["copy_notice"]["button"] == "Stop");
    assert!(preparing["app"]["copy_notice"]["text"].as_str().unwrap().contains("Preparing"), "{preparing}");
    run.ok(&["qa", "notice-button"]);
    run.settled();
    let stopped = wait_for(&run, |state| state["printing"]["busy"] == false);
    assert!(stopped["app"]["copy_notice"]["button"].is_null(), "{stopped}");
    assert!(!out.exists(), "nothing printed");
    let pid = stopped["printing"]["pid"].as_u64().unwrap();
    let left: Vec<String> = std::fs::read_dir(std::env::temp_dir()).unwrap().flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).filter(|name| name.starts_with(&format!("recto-print-{pid}-"))).collect();
    assert!(left.is_empty(), "its file goes: {left:?}");
}

/// Failure: going to a Find hit by a click on its row or Return in the field leaves no mark for Back, so ⌘[ can't bring the reader to where they were reading, as it can after a link; or ⌘G's step, which only steps, marks the trail or takes the way forward away.
#[test]
fn a_find_hit_clicked_or_returned_to_is_a_jump_and_a_step_isnt() {
    let Some(run) = Run::start() else { return };
    let file = numbered(200);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let go = |state: &Value, id: &str| menu_item(state, id)["enabled"].clone();
    run.ok(&["qa", "menu", "find.open"]);
    run.ok(&["qa", "find-type", "words"]);
    run.settled();
    assert_eq!(go(&run.report(), "go.back"), json!(false), "nothing to go back to yet");
    // Row 99 is page 50's hit: a page row, then its hit, for each page.
    run.ok(&["qa", "find-click", "99"]);
    let clicked = wait_for(&run, |state| state["app"]["place"] == "50 of 200");
    assert_eq!(go(&clicked, "go.back"), json!(true), "a click on a hit is a jump: {}", clicked["app"]["find"]);
    run.ok(&["qa", "menu", "go.back"]);
    let back = wait_for(&run, |state| state["app"]["place"] == "1 of 200");
    assert_eq!(go(&back, "go.forward"), json!(true));
    run.ok(&["qa", "menu", "find.next"]);
    let stepped = wait_for(&run, |state| state["app"]["place"] != "1 of 200");
    assert_eq!(go(&stepped, "go.forward"), json!(true), "⌘G only steps, so the way forward stays: {}", stepped["app"]["place"]);
    run.ok(&["qa", "find-key", "return"]);
    run.settled();
    let returned = run.report();
    assert_eq!((go(&returned, "go.back"), go(&returned, "go.forward")), (json!(true), json!(false)), "Return is a jump, which takes the way forward: {}", returned["app"]["place"]);
}

/// Failure: a step asked for before a hit for it is known (⇧⌘G with Find closed, from the first page, so the last hit waits for the query's end) is done when the query has more while Find's field is still locked, and the session thread waits on itself; the UI thread then hangs on its next look at Find (found from the palette's Find Next).
#[test]
fn a_step_kept_for_the_querys_end_is_done_without_a_hang() {
    let Some(run) = Run::start() else { return };
    let file = numbered(300);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "menu", "find.open"]);
    run.ok(&["qa", "find-type", "words"]);
    run.ok(&["qa", "find-key", "escape"]);
    wait_for(&run, |state| state["app"]["find"]["open"] == false);
    // Opens Find with what was typed, finds it again from the start, and goes back round to the last hit once the end is known.
    run.ok(&["qa", "menu", "find.previous"]);
    let went = wait_for(&run, |state| state["app"]["place"] == "300 of 300");
    assert_eq!(went["app"]["find"]["status"], "300 matches on 300 pages", "{}", went["app"]["find"]);
}

/// Failure: the person's own Find in a document they opened with its password finds nothing, or says nothing about why, because it reads pages with no password; or it gets there by writing the decrypted text to a corpus or index; or an agent's `recto find` gets its hits, at any level, as status or with a query.
#[test]
fn find_in_a_protected_document_works_for_the_person_and_never_for_an_agent() {
    let Some(run) = Run::start() else { return };
    let file = TempPdf::new("locked.pdf", &common::locked_with("open sesame", 2));
    run.ok(&["qa", "open-file", file.0.to_str().unwrap()]);
    sheet_of(&run, "password");
    type_password(&run, "open sesame");
    wait_for(&run, |state| state["app"]["sheet"] == json!({}) && state["app"]["place_label"] == "1 of 2");
    run.settled();
    run.ok(&["qa", "menu", "find.open"]);
    wait_for(&run, |state| state["app"]["find"]["open"] == true);
    run.ok(&["qa", "find-type", "locked page"]);
    let found = wait_for(&run, |state| state["app"]["find"]["status"].as_str().is_some_and(|status| status.contains("2 matches")));
    assert_eq!(found["app"]["find"]["rows"], 4, "two pages, a hit on each, read through the password: {}", found["app"]["find"]);
    // Nothing decrypted was written: no corpus file anywhere in the state folder holds the pages' words.
    assert!(files_holding(&run.state.root, b"Locked page").is_empty() && files_holding(&run.state.root, b"locked page").is_empty(), "decrypted text is on disk");
    for level in [Level::ReadOnly, Level::Control, Level::Full] {
        run.state.set_level(level);
        for args in [&["find"][..], &["find", "Locked"], &["find", "--next"]] {
            let (reply, code) = run.state.json(args);
            assert!(!reply.to_string().contains("ocked page"), "{args:?} at {level:?} gave a hit: {reply}");
            assert!([exit::PERMISSION, exit::PROTECTED].contains(&code), "{args:?} at {level:?}: {reply}");
        }
    }
    run.state.set_level(Level::Full);
}
