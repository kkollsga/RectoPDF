//! The state record end to end: the real app in quiet runs, quit and started again on the same temp state folder. SPEC decision 23: "It's crucial that app state is perfectly recovered before and after update".
//!
//! The first test here is the one that matters. It compares the whole of `qa state` before and after, with a short named list of exceptions, and a list of every path that's known about, so a field added to `qa state` fails until someone says here whether it's carried over or an exception. State is complete by construction, and nothing can be quietly forgotten.

mod common;

use common::State;
use recto_agent::Level;
use recto_platform::local::process_counters;
use recto_testpdf::{Object, Page, TempPdf, build, form_bomb, pages, text_lines};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

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

/// A quiet run on a state folder that outlives it.
struct Run<'a> {
    state: &'a State,
    pid: u32,
    done: bool,
}

impl<'a> Run<'a> {
    fn start(state: &'a State) -> Option<Self> {
        let app = app_path()?;
        let (reply, code) = state.json(&["qa", "start", "--app", app.to_str().unwrap(), "--time-limit", "3m"]);
        assert_eq!(code, 0, "{reply}");
        Some(Self { state, pid: reply["data"]["pid"].as_u64().unwrap() as u32, done: false })
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

    fn wait_gone(&mut self) {
        let began = Instant::now();
        while process_counters(self.pid).is_some() {
            assert!(began.elapsed() < Duration::from_secs(15), "the app didn't end");
            std::thread::sleep(Duration::from_millis(10));
        }
        // A process that can't be asked about any more can still be closing its sockets. It's gone when nothing answers there.
        while std::os::unix::net::UnixStream::connect(&self.state.paths.socket).is_ok() {
            assert!(began.elapsed() < Duration::from_secs(15), "the socket stayed");
            std::thread::sleep(Duration::from_millis(10));
        }
        self.done = true;
    }

    /// What the Quit menu item does.
    fn quit(mut self) {
        let _ = self.state.json(&["qa", "quit", "now"]);
        self.wait_gone();
    }

    /// What an update does.
    fn hand_off(mut self) -> Value {
        let (reply, code) = self.state.json(&["qa", "handoff", "now"]);
        assert_eq!(code, 0, "{reply}");
        self.wait_gone();
        reply["data"].clone()
    }

    /// What a crash does.
    fn kill(mut self) {
        let _ = Command::new("/bin/kill").arg("-9").arg(self.pid.to_string()).status();
        self.wait_gone();
    }
}

impl Drop for Run<'_> {
    fn drop(&mut self) {
        if !self.done {
            let _ = Command::new("/bin/kill").arg("-9").arg(self.pid.to_string()).status();
        }
    }
}

fn long_document(name: &str, count: usize) -> TempPdf {
    let list: Vec<Page> = (0..count).map(|index| Page::new(text_lines(&[&format!("{name} page {index}"), "Words, so that the page isn't blank"]))).collect();
    TempPdf::new(&format!("{name}.pdf"), &pages(&list))
}

/// A document with chapters and sections, so the sidebar has contents that scroll.
fn chaptered(name: &str, chapters: usize, sections: usize) -> TempPdf {
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
    // Each with something on it, wider down the document, so a shot shows which it is.
    let contents = first_page + total;
    for index in 0..total {
        objects.push(Object::Dict(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R >>", contents + index)));
    }
    for index in 0..total {
        objects.push(Object::Stream { dict: String::new(), data: format!("0.2 0.3 0.7 rg 72 640 {} 60 re f 0 g 72 560 460 14 re f 72 530 460 14 re f 72 500 300 14 re f", 40 + index * 2).into_bytes() });
    }
    TempPdf::new(&format!("{name}.pdf"), &build(&objects))
}

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

/// What's allowed to differ across a restart, and why. It's short on purpose: anything else that differs is a bug, or goes here with its reason.
const EXCEPTIONS: &[(&str, &str)] = &[
    ("memory.", "A new process's memory, threads and handles are its own: budgets, not state."),
    ("hangs.", "Its own stalls, counted from its own start."),
    ("record.", "How many writes it's made and what its first frame was: it's the record of the restore itself."),
    ("reader.counts.", "Jobs and steps of its own run."),
    ("reader.documents_held", "Warm tabs come back cold, by rule: only the active tab loads, so the rest cost what a cold tab costs."),
    ("reader.tiles.kept", "Tiles held off screen, which a new process doesn't have until it's been somewhere."),
    ("reader.tiles.previews_attached", "Previews behind sharp tiles, which depend on what the first draw was."),
    ("reader.tiles.attached", "Fewer are on layers if there's no preview under them: drawing, not state. What's visible is settled either way."),
    ("reader.tiles.page_layers", "Pages laid out around the view."),
    ("app.pages.page_layers", "The same, as the app has them."),
    ("app.pages.tile_layers", "The same, as the app has them."),
    ("app.pages.spare_layers", "A pool of its own."),
    ("app.pages.tint_layers", "One per page layer."),
    ("reader.tile_scale", "Comes from a zoom settled on, and a pinch at another zoom leaves it where it was."),
    ("app.contents.row_views", "Rows the table has made so far."),
    ("app.pages.pointer_steps", "Its own pointer moves, counted from its own start."),
    ("app.pages.worst_pointer_step_us", "The same, timed."),
    ("reader.selection.text_", "Page text held for a selection, which is read again when it's wanted."),
    ("reader.selection.unit", "What the last press took, words or lines. It says what a shift-click extends by, and nothing that shows."),
    ("app.copy_notice", "A line about a copy, which fades. A copy is over by then, or a hand-off waits for it."),
];

/// What can be compared as numbers. Scroll positions can differ by rounding only: the test below of pixels holds them much closer than any eye.
fn differs(left: &Value, right: &Value) -> bool {
    match (left.as_f64(), right.as_f64()) {
        (Some(left), Some(right)) => (left - right).abs() > 1e-6,
        _ => left != right,
    }
}

/// Compares everything, and fails on anything but what's named.
fn assert_same(before: &Value, after: &Value, known: &[&str]) {
    let (mut one, mut two) = (BTreeMap::new(), BTreeMap::new());
    flatten(before, "", &mut one);
    flatten(after, "", &mut two);
    let excepted = |path: &str| EXCEPTIONS.iter().any(|(name, _)| path.starts_with(name));
    let mut wrong = Vec::new();
    for (path, value) in &one {
        if excepted(path) {
            continue;
        }
        match two.get(path) {
            Some(other) if !differs(value, other) => {}
            other => wrong.push(format!("{path}: {value} then {}", other.map_or("nothing".to_string(), ToString::to_string))),
        }
    }
    for path in two.keys() {
        if !excepted(path) && !one.contains_key(path) {
            wrong.push(format!("{path}: nothing then {}", two[path]));
        }
    }
    assert!(wrong.is_empty(), "these changed across the restart:\n{}", wrong.join("\n"));
    if !known.is_empty() {
        let unknown: Vec<String> = one.keys().map(|path| general(path)).filter(|path| !known.contains(&path.as_str()) && !excepted(path)).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        assert!(unknown.is_empty(), "`qa state` has fields this test hasn't been told about. Carry each one over in the state record, or name it in EXCEPTIONS with why, then add it to KNOWN_PATHS:\n{}", unknown.join("\n"));
    }
}

/// Puts the app in a rich state: several tabs at various zoom and pages, folded contents, a wide sidebar, Find open with text typed, edit mode, unsaved rotate and delete with undo history by both owner and agent.
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
    run.ok(&["goto", "120"]);
    run.settled();
    run.ok(&["qa", "contents-scroll", "60"]);
    run.ok(&["qa", "scroll", "42"]);
    run.settled();
    // Everything else there is.
    run.ok(&["qa", "resize", "1300x820"]);
    run.ok(&["qa", "sidebar-width", "318"]);
    run.ok(&["qa", "panel", "toggle"]);
    run.ok(&["view", "--paper", "warm", "--agent-icon", "off"]);
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

/// Failure: anything the user can see or has changed doesn't survive a quit and a launch. **The test that matters most:** the whole of `qa state` is compared, across a normal quit, with a short named list of exceptions. A field added to `qa state` fails this until it's carried over or named, so state is complete by construction. In dark too, with light and dark screenshots to look at when `RECTO_RESTORE_SHOTS` names a folder.
#[test]
fn everything_comes_back_after_a_quit() {
    for (look, paper) in [("light", "warm"), ("dark", "dark")] {
        let state = State::new();
        let Some(run) = Run::start(&state) else { return };
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
        let run = Run::start(&state).unwrap();
        run.ok(&["qa", "scroller-style", "overlay"]);
        run.settled();
        let after = run.report();
        if let Some(folder) = &shots {
            let _ = std::fs::remove_file(folder.join(format!("after-{look}.png")));
            run.ok(&["qa", "shot", "--out", folder.join(format!("after-{look}.png")).to_str().unwrap()]);
        }
        assert_same(&before, &after, KNOWN_PATHS);
        // The exact pixel, at the same zoom, on the first frame.
        for field in ["scroll_y", "scroll_x", "magnification"] {
            let (was, now) = (before["app"]["pages"][field].as_f64().unwrap(), after["app"]["pages"][field].as_f64().unwrap());
            assert!((was - now).abs() < 0.25, "{field}: {was} then {now}");
        }
        assert_eq!(after["session"]["find"]["draft"], "typed but not run");
        assert_eq!(after["session"]["app"]["focus"]["view"], "find");
        // It's all still there to use: undo and redo with whose each is.
        run.ok(&["qa", "owner-edit", "redo"]);
        run.quit();
    }
}

/// Every path in `qa state` that's been looked at: it comes back, as the test above shows, or it's in [`EXCEPTIONS`]. A new one fails until it's added here, which is the moment to carry it over.
const KNOWN_PATHS: &[&str] = &[
    "app.activation",
    "app.agent_icon",
    "app.appearance",
    "app.backing",
    "app.contents.count",
    "app.contents.current",
    "app.contents.current_in_view",
    "app.contents.first_visible",
    "app.contents.following",
    "app.contents.footer_on_screen",
    "app.contents.frames_running",
    "app.contents.height",
    "app.contents.list_height",
    "app.contents.moving",
    "app.contents.reduce_motion",
    "app.contents.scroll_y",
    "app.contents.scroller",
    "app.contents.stale_rows",
    "app.contents.tab_rows_on_screen",
    "app.contents.table_rows",
    "app.contents.tabs_scroll",
    "app.contents.visible_rows",
    "app.focus",
    "app.focus_view",
    "app.home",
    "app.hover",
    "app.is_active",
    "app.menu_bar.len",
    "app.menu_bar[].items.len",
    "app.menu_bar[].items[].checked",
    "app.menu_bar[].items[].enabled",
    "app.menu_bar[].items[].id",
    "app.menu_bar[].items[].key",
    "app.menu_bar[].items[].title",
    "app.menu_bar[].title",
    "app.note",
    "app.pages.autoscrolling",
    "app.pages.cursor",
    "app.pages.document_height",
    "app.pages.document_width",
    "app.pages.highlight_layers",
    "app.pages.highlight_rects",
    "app.pages.horizontal_scroller",
    "app.pages.magnification",
    "app.pages.scroll_x",
    "app.pages.scroll_y",
    "app.pages.scroller_style",
    "app.pages.scroller_top_inset",
    "app.pages.shot_tint_left",
    "app.pages.system_scroller_style",
    "app.pages.view_width",
    "app.pages.visible",
    "app.panel.height",
    "app.panel.items.len",
    "app.panel.items[].id",
    "app.panel.items[].selected",
    "app.panel.items[].title",
    "app.panel.open",
    "app.paper",
    "app.place",
    "app.place_label",
    "app.reveal.row",
    "app.reveal.shown",
    "app.sidebar_rows",
    "app.sidebar_visible",
    "app.sidebar_width",
    "app.window.full_screen",
    "app.window.height",
    "app.window.is_visible",
    "app.window.on_any_screen",
    "app.window.width",
    "app.window.x",
    "app.window.y",
    "app.window_open",
    "reader.document_height",
    "reader.fit",
    "reader.low_res_pages.len",
    "reader.magnification",
    "reader.note",
    "reader.page",
    "reader.pages",
    "reader.paper",
    "reader.scroll_x",
    "reader.scroll_y",
    "reader.settled",
    "reader.selection.copy_waiting_for_host",
    "reader.selection.copying",
    "reader.selection.dragging",
    "reader.selection.end",
    "reader.selection.pages_with_highlight",
    "reader.selection.selected",
    "reader.selection.start",
    "reader.tab",
    "reader.visible_pages.len",
    "reader.visible_pages[]",
    "reader.window_open",
    "session.active",
    "session.app.appearance",
    "session.app.contents_scroll.following",
    "session.app.contents_scroll.y",
    "session.app.focus.app_frontmost",
    "session.app.focus.view",
    "session.app.focus.window_key",
    "session.app.notice",
    "session.app.panel_open",
    "session.app.sidebar_width",
    "session.app.window.frame.height",
    "session.app.window.frame.width",
    "session.app.window.frame.x",
    "session.app.window.frame.y",
    "session.app.window.full_screen",
    "session.app.window.screen",
    "session.edit_mode",
    "session.edits_full.N.added.len",
    "session.edits_full.N.added[].file.device",
    "session.edits_full.N.added[].file.inode",
    "session.edits_full.N.added[].file.mtime_ns",
    "session.edits_full.N.added[].file.size",
    "session.edits_full.N.added[].page_count",
    "session.edits_full.N.added[].path",
    "session.edits_full.N.file_pages",
    "session.edits_full.N.history_dropped",
    "session.edits_full.N.owner_step_dropped",
    "session.edits_full.N.pages.len",
    "session.edits_full.N.pages[].len",
    "session.edits_full.N.pages[][]",
    "session.edits_full.N.redo.len",
    "session.edits_full.N.redo[].by",
    "session.edits_full.N.redo[].step.remove.len",
    "session.edits_full.N.redo[].step.remove.start",
    "session.edits_full.N.redo[].step.rotate.len",
    "session.edits_full.N.redo[].step.rotate.quarter_turns",
    "session.edits_full.N.redo[].step.rotate.start",
    "session.edits_full.N.undo.len",
    "session.edits_full.N.undo[].by",
    "session.edits_full.N.undo[].step.move.len",
    "session.edits_full.N.undo[].step.move.start",
    "session.edits_full.N.undo[].step.move.to",
    "session.edits_full.N.undo[].step.remove.len",
    "session.edits_full.N.undo[].step.remove.start",
    "session.edits_full.N.undo[].step.restore.runs.len",
    "session.edits_full.N.undo[].step.restore.runs[].len",
    "session.edits_full.N.undo[].step.restore.runs[][]",
    "session.edits_full.N.undo[].step.restore.start",
    "session.edits_full.N.undo[].step.rotate.len",
    "session.edits_full.N.undo[].step.rotate.quarter_turns",
    "session.edits_full.N.undo[].step.rotate.start",
    "session.find.draft",
    "session.find.open",
    "session.find.query",
    "session.next_id",
    "session.tabs.len",
    "session.tabs[].edits",
    "session.tabs[].edits.edited",
    "session.tabs[].edits.holds_owner_edits",
    "session.tabs[].edits.owner_steps_undone",
    "session.tabs[].edits.pages",
    "session.tabs[].edits.redo",
    "session.tabs[].edits.undo",
    "session.tabs[].file.device",
    "session.tabs[].file.inode",
    "session.tabs[].file.mtime_ns",
    "session.tabs[].file.size",
    "session.tabs[].id",
    "session.tabs[].journal",
    "session.tabs[].notice",
    "session.tabs[].page",
    "session.tabs[].page_count",
    "session.tabs[].path",
    "session.tabs[].place.anchor_page",
    "session.tabs[].place.document_height",
    "session.tabs[].place.page",
    "session.tabs[].place.page_height",
    "session.tabs[].place.page_width",
    "session.tabs[].place.within",
    "session.tabs[].place.x",
    "session.tabs[].place.y",
    "session.tabs[].place.zoom",
    "session.tabs[].renderer_only",
    "session.tabs[].selection",
    "session.tabs[].toc_open",
    "session.tabs[].view_rotation",
    "session.view.agent_icon_hidden",
    "session.view.focus",
    "session.view.paper",
];

/// Failure: the first frame shows the wrong page or the home screen, or the window comes up empty and fills in. It's held back until the page is on its layers, and this is what was on it when it showed: the place, drawn.
#[test]
fn the_first_frame_is_the_right_one() {
    let state = State::new();
    let Some(run) = Run::start(&state) else { return };
    let file = long_document("first", 60);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "33"]);
    run.settled();
    run.ok(&["qa", "scroll", "71"]);
    run.settled();
    let before = run.report();
    run.quit();
    let run = Run::start(&state).unwrap();
    run.settled();
    let shown = run.report()["record"]["first_shown"].clone();
    assert_eq!(shown["home"], false, "{shown}");
    assert_eq!(shown["settled"], true, "the page was drawn before it showed: {shown}");
    assert!(shown["tile_layers"].as_u64().unwrap() > 0, "{shown}");
    assert_eq!(shown["page"], before["reader"]["page"]);
    assert!((shown["scroll_y"].as_f64().unwrap() - before["reader"]["scroll_y"].as_f64().unwrap()).abs() < 0.25, "{shown} {}", before["reader"]);
}

/// Failure: an update mid-work loses what the reader had going, or asks them something. It hands off in the middle of Find text typed and edit mode on, and the whole report comes back. It never prompts: there's nothing to answer in a quiet run, and it ends by itself.
#[test]
fn a_hand_off_in_the_middle_of_things_loses_nothing() {
    let state = State::new();
    let Some(run) = Run::start(&state) else { return };
    let (a, b, c) = (long_document("alpha", 60), long_document("beta", 30), chaptered("gamma", 20, 8));
    run.ok(&["qa", "scroller-style", "overlay"]);
    make_rich(&run, &[&a, &b, &c]);
    let before = run.report();
    let flushed = run.hand_off();
    assert_eq!(flushed["handed_off"], true);
    assert_eq!(flushed["edited_tabs"], 1, "{flushed}");
    let run = Run::start(&state).unwrap();
    run.ok(&["qa", "scroller-style", "overlay"]);
    run.settled();
    let after = run.report();
    assert_same(&before, &after, KNOWN_PATHS);
    assert_eq!(after["session"]["edit_mode"], true);
    assert_eq!(after["session"]["find"]["draft"], "typed but not run");
}

/// Failure: an update goes ahead with a sheet open or an agent's command running, so it's lost. The check and the hand-off both say what's in the way, and it can go once that's over. How long since the user touched the app is one number, there to ask, with what touches it resetting it.
#[test]
fn a_hand_off_waits_for_what_is_in_flight() {
    let state = State::new();
    let Some(run) = Run::start(&state) else { return };
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
    let Some(run) = Run::start(&state) else { return };
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
    let Some(run) = Run::start(&state) else { return };
    let file = long_document("first-edit", 12);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "7"]);
    run.settled();
    // The record goes out, and then nothing else does for a minute.
    run.ok(&["qa", "frontmost", "off"]);
    run.quit();
    let run = Run::start(&state).unwrap();
    run.settled();
    run.ok(&["qa", "owner-edit", "rotate:2"]);
    let lists = wait_for_journal(&run);
    run.kill();
    let run = Run::start(&state).unwrap();
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
    let Some(run) = Run::start(&state) else { return };
    let file = long_document("no-record", 9);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "owner-edit", "delete:3"]);
    let lists = wait_for_journal(&run);
    assert!(!state.root.join("session/record.a.json").exists() && !state.root.join("session/record.b.json").exists(), "this is about having no record");
    run.kill();
    let run = Run::start(&state).unwrap();
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
    let Some(run) = Run::start(&state) else { return };
    let file = long_document("closing", 6);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["page", "rotate", "1", "--by", "90"]);
    wait_for_journal(&run);
    run.ok(&["undo"]);
    run.ok(&["close"]);
    run.quit();
    let run = Run::start(&state).unwrap();
    run.settled();
    let after = run.report();
    assert_eq!(tabs_of(&after).len(), 0, "{}", after["session"]["tabs"]);
    // And the PDF opened again has its way forward.
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let list = run.report()["session"]["edits_full"].as_object().unwrap().values().next().cloned().expect("its history");
    assert_eq!((list["undo"].as_array().unwrap().len(), list["redo"].as_array().unwrap().len()), (0, 1), "{list}");
}

/// Failure (review B4): owner history with nothing unsaved, as four quarter turns are, is what a close allows and keeps. It brought the tab back on every launch, under a new number each time, however many times it was closed. A closed tab stays closed, and its history waits for the PDF to be opened.
#[test]
fn a_closed_tab_stays_closed_and_keeps_its_history() {
    let state = State::new();
    let Some(run) = Run::start(&state) else { return };
    let file = long_document("closed", 6);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    for _ in 0..4 {
        run.ok(&["qa", "owner-edit", "rotate:1"]);
    }
    run.ok(&["close"]);
    run.quit();
    for _ in 0..2 {
        let run = Run::start(&state).unwrap();
        run.settled();
        assert_eq!(tabs_of(&run.report()).len(), 0);
        run.quit();
    }
    let run = Run::start(&state).unwrap();
    run.settled();
    // By the app's own route: a drop, the menu, a launch argument.
    run.ok(&["qa", "open-file", file.0.to_str().unwrap()]);
    run.settled();
    let list = run.report()["session"]["edits_full"].as_object().unwrap().values().next().cloned().expect("its history");
    assert_eq!(list["undo"].as_array().unwrap().len(), 4, "{list}");
}

/// Failure (review B5): `recto open` didn't pick up a closed PDF's history as the app's own open did, so the next edit went on the end of the old journal. What was on disk stopped matching what was in memory, and every save and hand-off was refused. The same route either way, by rule 4, and the same history.
#[test]
fn opening_by_either_route_picks_up_the_history() {
    let state = State::new();
    let Some(run) = Run::start(&state) else { return };
    let file = long_document("routes", 8);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    for _ in 0..4 {
        run.ok(&["qa", "owner-edit", "rotate:1"]);
    }
    run.ok(&["close"]);
    run.quit();
    let run = Run::start(&state).unwrap();
    run.settled();
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "owner-edit", "delete:5"]);
    let list = run.report()["session"]["edits_full"].as_object().unwrap().values().next().cloned().expect("a list");
    assert_eq!(list["undo"].as_array().unwrap().len(), 5, "its 4 and the new one: {list}");
    run.ok(&["session", "save"]);
    let lists = run.report()["session"]["edits_full"].clone();
    run.kill();
    let run = Run::start(&state).unwrap();
    run.settled();
    let back = run.report()["session"]["edits_full"].clone();
    assert_eq!(back.as_object().unwrap().values().next(), lists.as_object().unwrap().values().next());
    assert_eq!(tabs_of(&run.report()).len(), 1);
}

/// Failure (decision 26, and review B7): a second tab through a link has the same file, so it shared a journal: an edit on it landed in the first's file, and after a restart the first had it and the second was unreadable. "A PDF can only be opened once": a link goes to the tab, by any route, and `recto open` says so. A copy is another file, and a PDF added as pages isn't a tab.
#[test]
fn a_pdf_can_only_be_opened_once_by_any_route() {
    let state = State::new();
    let Some(run) = Run::start(&state) else { return };
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
    let Some(run) = Run::start(&state) else { return };
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let (original, copy) = (folder.join("a.pdf"), folder.join("acopy.pdf"));
    std::fs::write(&original, pages(&(0..6).map(|at| Page::new(text_lines(&[&format!("Page {at}")]))).collect::<Vec<_>>())).unwrap();
    assert!(Command::new("/bin/cp").arg("-p").arg(&original).arg(&copy).status().unwrap().success());
    run.ok(&["open", original.to_str().unwrap()]);
    run.ok(&["open", copy.to_str().unwrap()]);
    run.settled();
    // Both in the record, then nothing else goes for a minute.
    run.quit();
    let run = Run::start(&state).unwrap();
    run.settled();
    let tabs = tabs_of(&run.report());
    assert_eq!(tabs.len(), 2);
    let on_copy = tabs.iter().find(|tab| tab["path"].as_str().unwrap().ends_with("acopy.pdf")).unwrap()["id"].as_u64().unwrap();
    run.ok(&["tap", &format!("#{on_copy}")]);
    run.settled();
    run.ok(&["qa", "owner-edit", "delete:3"]);
    wait_for_journal(&run);
    run.kill();
    let run = Run::start(&state).unwrap();
    run.settled();
    let after = run.report();
    assert_eq!(tabs_of(&after).len(), 2, "{}", after["session"]["tabs"]);
    let lists = after["session"]["edits_full"].as_object().unwrap();
    assert_eq!(lists.keys().collect::<Vec<_>>(), [&on_copy.to_string()], "on the copy, where they were made: {}", after["session"]["tabs"]);
}

/// Failure (final review N7): a closed PDF's history is taken up when its file couldn't even be read, so it's marked as open with nothing holding it. Opening again and editing wrote over the end of the old journal, and every save and hand-off failed with what's written not reading back. It's only taken after a good read.
#[test]
fn history_is_only_taken_up_after_a_good_read() {
    use std::os::unix::fs::PermissionsExt;
    let state = State::new();
    let Some(run) = Run::start(&state) else { return };
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("a.pdf");
    std::fs::write(&file, pages(&(0..6).map(|at| Page::new(text_lines(&[&format!("Page {at}")]))).collect::<Vec<_>>())).unwrap();
    run.ok(&["open", file.to_str().unwrap()]);
    run.settled();
    for _ in 0..4 {
        run.ok(&["qa", "owner-edit", "rotate:1"]);
    }
    run.ok(&["close"]);
    run.ok(&["session", "save"]);
    let began = Instant::now();
    while !std::fs::read_dir(state.root.join("session/journals")).unwrap().flatten().any(|entry| entry.file_name().to_string_lossy().starts_with("closed-")) {
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
    let list = run.report()["session"]["edits_full"].as_object().unwrap().values().next().cloned().expect("a list");
    assert_eq!(list["undo"].as_array().unwrap().len(), 5, "its 4 and the new one: {list}");
    run.ok(&["session", "save"]);
    run.hand_off();
}

/// Failure (final review N4): a quit during a read that's stuck wrote nothing at all, as what's right for a hand-off isn't for a quit: there, the update waits, but here the app's going anyway. The reader's place came back as page 1. A quit writes what there is.
#[test]
fn a_quit_during_a_stuck_read_still_writes() {
    let state = State::new();
    let Some(run) = Run::start(&state) else { return };
    let (file, other) = (long_document("stuck", 20), long_document("other", 2));
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "hold-reads", "on"]);
    run.ok(&["qa", "open-file", other.0.to_str().unwrap()]);
    run.ok(&["goto", "12"]);
    run.quit();
    let run = Run::start(&state).unwrap();
    run.settled();
    assert_eq!(run.report()["reader"]["page"], 12);
}

/// Failure (review T12, T13): the damaged-state notice is saved and shown again on every launch, though the reader has seen it. And what a record's notice says reaches the reader and agents as text, forged or not, in a field documented as Rust's own. It's a code, it goes once they've moved on, and a code it doesn't know says nothing.
#[test]
fn a_notice_goes_once_it_has_been_read_and_cant_be_forged() {
    use recto_session::record;
    let state = State::new();
    let Some(run) = Run::start(&state) else { return };
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
    let run = Run::start(&state).unwrap();
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
    let run = Run::start(&state).unwrap();
    assert!(run.report()["app"]["note"].as_str().unwrap().contains("couldn't be read"));
    run.ok(&["qa", "note-input", "now"]);
    let began = Instant::now();
    while !run.report()["app"]["note"].is_null() {
        assert!(began.elapsed() < Duration::from_secs(5), "it stayed");
        std::thread::sleep(Duration::from_millis(20));
    }
    run.quit();
    let run = Run::start(&state).unwrap();
    assert_eq!(run.report()["app"]["note"], Value::Null, "and it doesn't come back");
}

/// Failure (review T1, T2): an update goes ahead with an agent's command running or a file being read, whose work is then lost. A page that takes seconds to fail holds the command, and QA holds the read back. Each is named, the hand-off refuses, and it goes once they've ended. And what was refused leaves nothing frozen.
#[test]
fn a_hand_off_waits_for_a_command_and_a_read() {
    let state = State::new();
    let Some(run) = Run::start(&state) else { return };
    let file = long_document("handed", 4);
    let (other, bomb) = (long_document("other", 2), TempPdf::new("bomb.pdf", &form_bomb()));
    run.ok(&["open", bomb.0.to_str().unwrap()]);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    // An agent's command, under way on a job thread.
    let out = state.root.join("bomb.png");
    let mut waiting = state.command().args(["render", "1", "--tab", "#1", "--out", out.to_str().unwrap(), "--json"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
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

/// Failure: what's selected is lost across a restart: SPEC decision 23 has it as part of what comes back. On the page it was on, the same characters, and highlighted again.
#[test]
fn a_selection_comes_back() {
    let state = State::new();
    let Some(run) = Run::start(&state) else { return };
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
    let run = Run::start(&state).unwrap();
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
    let Some(run) = Run::start(&state) else { return };
    let folder = state.root.join("docs");
    std::fs::create_dir_all(folder.join("there")).unwrap();
    let path = folder.join("a.pdf");
    std::fs::write(&path, pages(&(0..6).map(|at| Page::new(text_lines(&[&format!("Page {at}")]))).collect::<Vec<_>>())).unwrap();
    run.ok(&["open", path.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "owner-edit", "rotate:2"]);
    run.ok(&["qa", "scroller-style", "overlay"]);
    run.quit();
    std::fs::rename(&path, folder.join("there/b.pdf")).unwrap();
    let run = Run::start(&state).unwrap();
    run.ok(&["qa", "scroller-style", "overlay"]);
    run.settled();
    let before = run.report();
    assert_eq!(before["session"]["tabs"][0]["notice"]["moved"]["from"], path.to_str().unwrap(), "{}", before["session"]["tabs"]);
    run.quit();
    let run = Run::start(&state).unwrap();
    run.ok(&["qa", "scroller-style", "overlay"]);
    run.settled();
    assert_same(&before, &run.report(), &[]);
}

/// Failure: killed outright mid-work, the app comes back with nothing, or a broken record. What had gone to disk by then is there: edits past their wait, whose each is, and the record's whole however it was caught.
#[test]
fn killed_outright_it_comes_back_with_what_was_written() {
    let state = State::new();
    let Some(run) = Run::start(&state) else { return };
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
    let run = Run::start(&state).unwrap();
    run.settled();
    let after = run.report();
    let mut lists_back = BTreeMap::new();
    for (_, list) in after["session"]["edits_full"].as_object().unwrap() {
        lists_back.insert("one", list.clone());
    }
    assert_eq!(lists_back.get("one"), lists.as_object().unwrap().values().next(), "the edits and whose they were");
    assert_eq!(after["app"]["note"], Value::Null, "it isn't reported as damaged");
    let lists = after["session"]["edits_full"].clone();
    // Killed again at once, with something on its way: still whole.
    run.ok(&["goto", "3"]);
    run.kill();
    let run = Run::start(&state).unwrap();
    run.settled();
    assert_eq!(run.report()["session"]["edits_full"], lists);
    run.quit();
}

/// Failure: going round twice loses something the first time didn't: a record written from a restored session differs from the one before it. Quit, start, quit, start, and all three are the same.
#[test]
fn quitting_twice_in_a_row_changes_nothing() {
    let state = State::new();
    let Some(run) = Run::start(&state) else { return };
    let (a, b, c) = (long_document("alpha", 60), long_document("beta", 30), chaptered("gamma", 20, 8));
    run.ok(&["qa", "scroller-style", "overlay"]);
    make_rich(&run, &[&a, &b, &c]);
    let first = run.report();
    run.quit();
    let run = Run::start(&state).unwrap();
    run.ok(&["qa", "scroller-style", "overlay"]);
    run.settled();
    let second = run.report();
    run.quit();
    let run = Run::start(&state).unwrap();
    run.ok(&["qa", "scroller-style", "overlay"]);
    run.settled();
    let third = run.report();
    assert_same(&first, &second, &[]);
    assert_same(&second, &third, &[]);
}

/// Failure: a record of 1,000 tabs comes back slowly, or with a handle or a thread for each, or wakes them all. Only the active tab holds a document, and it has no more handles or threads than 2 tabs do, but for the one read of the active tab's file and the one look at the others, which may not have ended yet.
#[test]
fn a_thousand_tabs_come_back_cold() {
    let state = State::new();
    let Some(run) = Run::start(&state) else { return };
    let file = long_document("many", 3);
    let folder = state.root.join("tabs");
    std::fs::create_dir_all(&folder).unwrap();
    let link = |at: usize| folder.join(format!("t{at}.pdf"));
    // Files of their own, as a link is the same PDF.
    for at in 0..1000 {
        recto_platform::local::clone_file(&file.0, &link(at)).unwrap();
    }
    run.ok(&["open", link(0).to_str().unwrap()]);
    run.ok(&["open", link(1).to_str().unwrap()]);
    run.settled();
    let few = run.report();
    run.quit();
    // Those two came back. The rest go in as a record, as though they'd been open.
    let run = Run::start(&state).unwrap();
    run.settled();
    let two = run.report();
    assert_eq!(two["session"]["tabs"].as_array().unwrap().len(), 2);
    for at in 2..1000 {
        run.ok(&["open", link(at).to_str().unwrap()]);
    }
    run.settled();
    let opened = run.report();
    run.quit();
    let started = Instant::now();
    let run = Run::start(&state).unwrap();
    run.settled();
    let took = started.elapsed();
    let back = run.report();
    assert_eq!(back["session"]["tabs"].as_array().unwrap().len(), 1000);
    assert_eq!(back["reader"]["documents_held"], 1, "only the active tab");
    assert_eq!(back["session"]["active"], opened["session"]["active"]);
    let (threads_few, threads_back) = (few["memory"]["threads"].as_u64().unwrap(), back["memory"]["threads"].as_u64().unwrap());
    assert!(threads_back <= threads_few + 2, "{threads_few} then {threads_back} threads");
    let (handles_few, handles_back) = (few["memory"]["open_handles"].as_u64().unwrap(), back["memory"]["open_handles"].as_u64().unwrap());
    assert!(handles_back <= handles_few + 2, "{handles_few} then {handles_back} handles");
    eprintln!("1,000 tabs came back and settled in {took:?}");
}

/// Failure: a damaged record stops the app, panics, is overwritten, or is passed over in silence. It starts empty, says so, and what was there is kept to one side.
#[test]
fn a_damaged_record_starts_empty_and_says_so() {
    let state = State::new();
    let Some(run) = Run::start(&state) else { return };
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
    let run = Run::start(&state).unwrap();
    let after = run.report();
    assert_eq!(after["app"]["home"], true);
    assert!(after["app"]["note"].as_str().unwrap().contains("couldn't be read"), "{}", after["app"]["note"]);
    assert_eq!(std::fs::read(state.root.join("session.damaged/record.a.json")).unwrap(), b"{\"not\": \"a record\"}");
}

/// Failure: a record from a newer version gets overwritten, so the update that wrote it finds it gone when it comes back. Left untouched, byte for byte, an empty start, and a notice.
#[test]
fn a_newer_record_is_left_alone() {
    let state = State::new();
    state.paths.prepare().unwrap();
    let folder = state.root.join("session");
    recto_platform::local::ensure_private_dir(&folder).unwrap();
    let newer = recto_session::record::seal_as(recto_session::record::MAIN_KIND, recto_session::record::CURRENT_VERSION + 1, 9, b"from the future");
    recto_platform::local::write_private_file(&folder.join("record.a.json"), &newer).unwrap();
    let Some(run) = Run::start(&state) else { return };
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
    let Some(run) = Run::start(&state) else { return };
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
    let run = Run::start(&state).unwrap();
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
    let codes: Vec<&str> = show["tabs"].as_array().unwrap().iter().map(|tab| tab["notice"]["code"].as_str().unwrap()).collect();
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

/// Failure: restoring gives an agent more than it had: a look at Find's text below Control, or a way to throw away the owner's edits by restoring over them, or a level from the record. And a session's ownership rule stops holding once it's been restarted: below Full an agent still can't throw away the owner's edits, and it can its own.
#[test]
fn levels_and_ownership_hold_after_a_restart() {
    let state = State::new();
    let Some(run) = Run::start(&state) else { return };
    let (a, b) = (long_document("owners", 8), long_document("agents", 8));
    run.ok(&["open", a.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "owner-edit", "rotate:1"]);
    run.ok(&["open", b.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["page", "rotate", "1", "--by", "90"]);
    run.ok(&["qa", "find-query", "something private"]);
    run.quit();
    let run = Run::start(&state).unwrap();
    run.settled();
    state.set_level(Level::Control);
    assert_eq!(run.ok(&["where"])["tab"], 2);
    // The agent's own.
    run.ok(&["discard"]);
    // The owner's.
    run.ok(&["tap", "#1"]);
    run.settled();
    let (reply, code) = state.json(&["discard"]);
    assert_eq!(code, recto_agent::exit::PERMISSION, "{reply}");
    let (reply, code) = state.json(&["session", "restore"]);
    assert_eq!(code, recto_agent::exit::PERMISSION, "restoring over the owner's edits: {reply}");
    // A level is only ever in its own file: a record saying otherwise is nothing, as an unknown field.
    assert_eq!(state.json(&["permission", "show"]).0["data"]["level"], "control");
    assert_eq!(run.ok(&["session", "show"])["find"]["query"], "something private");
    state.set_level(Level::ReadOnly);
    let show = run.ok(&["session", "show"]);
    assert!(show["find"].get("query").is_none(), "not below Control: {show}");
    assert_eq!(show["find"]["has_text"], true);
    assert_ne!(state.json(&["session", "save"]).1, 0);
    state.set_level(Level::Off);
    assert_ne!(state.json(&["session", "show"]).1, 0);
}

/// Failure: reading writes to the disk all the time, wearing it for nothing: SPEC's budget. At rest, 0 bytes. Scrolling for a while, at most a line, as a minute hasn't passed. A burst of edits, about what the steps come to and not a rewrite per step. Going to the background writes what changed, and nothing after that when nothing has.
#[test]
fn it_writes_only_what_happened() {
    let state = State::new();
    let Some(run) = Run::start(&state) else { return };
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
