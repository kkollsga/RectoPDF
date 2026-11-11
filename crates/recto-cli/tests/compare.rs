//! Comparison view end to end: the real app in a quiet run, driven by what the UI calls. Owner, 2026-11-08: "Ctrl clicking more then one pdf should enable comparison view, right click menu can enable separate scroll (default) and synced scroll. Zoom is always synced."
//!
//! What comes back after a restart and an update is in `restore.rs`, whose whole-state comparison has a comparison in it.

mod common;

use common::State;
use common::app::Run;
use recto_testpdf::{Page, TempPdf, pages, text_lines};
use serde_json::Value;
use std::time::{Duration, Instant};

/// A document whose every page says which document and page it is, at a page size of its own.
fn document(name: &str, count: usize, media: (f32, f32)) -> TempPdf {
    let list: Vec<Page> = (0..count)
        .map(|index| Page {
            media,
            ..Page::new(text_lines(&[&format!("{name}word page {index}"), "Words, so that the page isn't blank"]))
        })
        .collect();
    TempPdf::new(&format!("{name}.pdf"), &pages(&list))
}

fn panes(report: &Value) -> Vec<Value> {
    report["panes"]["panes"].as_array().unwrap().clone()
}

fn page_of(report: &Value, tab: u64) -> i64 {
    panes(report)
        .iter()
        .find(|pane| pane["tab"] == tab)
        .map(|pane| pane["page"].as_i64().unwrap())
        .unwrap_or(-1)
}

/// Failure: ⌘-click doesn't put PDFs side by side, a pane runs into the next or has no name on it, the right-click menu has no scroll choice, synced scroll doesn't keep the panes page for page from where it went on or says nothing of different lengths, zoom differs between panes, a selection or a copy isn't the pane being read's, closing a pane's tab leaves its page view behind with its thread and worker, or a plain click doesn't return to one. Mixed page sizes and lengths, and side by side inside a pane, as the brief asks.
#[test]
fn comparison_view_from_command_click_to_one_again() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let (a, b, c) = (
        document("alpha", 40, (612.0, 792.0)),
        document("beta", 12, (842.0, 595.0)),
        document("gamma", 25, (595.0, 842.0)),
    );
    run.ok(&["open", a.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "10"]);
    for file in [&b, &c] {
        run.ok(&["open", file.0.to_str().unwrap()]);
    }
    run.ok(&["qa", "scroller-style", "overlay"]);
    run.settled();
    let one = run.report();
    let threads_alone = one["memory"]["threads"].as_u64().unwrap();
    assert_eq!(panes(&one).len(), 1);
    assert!(
        one["context_menu"].is_null()
            && one["app"]["context_menu"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| !item["id"].as_str().unwrap().starts_with("compare.")),
        "no scroll choice with one tab"
    );

    // ⌘-click on the first tab's row, with gamma showing: side by side, alpha read.
    run.ok(&["qa", "compare-click", "0"]);
    run.settled();
    let two = run.report();
    let shown: Vec<u64> = panes(&two).iter().map(|pane| pane["tab"].as_u64().unwrap()).collect();
    assert_eq!(shown, vec![3, 1], "{}", two["panes"]);
    assert_eq!(two["session"]["active"], 1, "the one clicked is read");
    assert_eq!(two["app"]["place"], "10 of 40", "the page number is the pane being read's");
    let app_panes = two["app"]["panes"].as_array().unwrap();
    assert_eq!(app_panes.len(), 2);
    for pane in app_panes {
        assert_eq!(pane["pages_inside"], true, "{pane}");
        assert_eq!(pane["label_in_lower_left"], true, "{pane}");
    }
    assert_eq!(app_panes[0]["label"], "gamma");
    assert_eq!(app_panes[1]["reading"], true);
    assert!(app_panes[0]["x"].as_f64().unwrap() + app_panes[0]["width"].as_f64().unwrap() <= app_panes[1]["x"].as_f64().unwrap());
    let magnifications: Vec<f64> = panes(&two).iter().map(|pane| pane["magnification"].as_f64().unwrap()).collect();
    assert!((magnifications[0] - magnifications[1]).abs() < 1e-9, "one zoom: {magnifications:?}");
    assert_eq!(
        two["app"]["zoom_level"]["clear_of_pages"], true,
        "the corner stays clear in the pane at the right"
    );
    assert_eq!(two["app"]["agent_icon_margins"]["clear_of_pages"], true);
    let menu: Vec<(String, bool)> = two["app"]["context_menu"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["id"].as_str().unwrap().starts_with("compare."))
        .map(|item| (item["id"].as_str().unwrap().to_string(), item["checked"].as_bool().unwrap()))
        .collect();
    assert_eq!(
        menu,
        [("compare.separate".to_string(), true), ("compare.synced".to_string(), false)],
        "separate by default"
    );

    // Separate: a scroll in one leaves the other.
    let gamma_was = page_of(&two, 3);
    run.ok(&["qa", "pane-scroll", "1:3000"]);
    run.settled();
    assert_eq!(page_of(&run.report(), 3), gamma_was, "separate scroll moves one pane");

    // Synced, from the right-click menu, then a scroll in gamma's pane leads, and alpha follows page for page.
    run.ok(&["qa", "context-menu", "compare.synced"]);
    run.settled();
    let synced = run.report();
    assert_eq!(synced["session"]["comparison"]["synced"], true);
    assert!(
        synced["app"]["note"].as_str().is_some_and(|note| note.contains("25 and 40")),
        "different lengths are said: {}",
        synced["app"]["note"]
    );
    let (gamma_start, alpha_start) = (page_of(&synced, 3), page_of(&synced, 1));
    run.ok(&["qa", "pane-scroll", "0:4000"]);
    run.settled();
    let led = run.report();
    let (gamma_now, alpha_now) = (page_of(&led, 3), page_of(&led, 1));
    assert!(gamma_now > gamma_start, "the pane scrolled: {gamma_start} to {gamma_now}");
    assert!(
        ((alpha_now - alpha_start) - (gamma_now - gamma_start)).abs() <= 1,
        "page for page: gamma {gamma_start}→{gamma_now}, alpha {alpha_start}→{alpha_now}"
    );
    assert_eq!(led["session"]["active"], 3, "scrolling a pane makes it the one read");

    // Zoom in the pane being read is zoom in each.
    run.ok(&["qa", "zoom", "1.5"]);
    run.settled();
    let zoomed = run.report();
    assert!(
        panes(&zoomed).iter().all(|pane| (pane["magnification"].as_f64().unwrap() - 1.5).abs() < 1e-6),
        "{}",
        zoomed["panes"]
    );
    run.ok(&["view", "--zoom", "fit"]);
    run.settled();

    // A third, then out again by the same click.
    run.ok(&["qa", "compare-click", "1"]);
    run.settled();
    assert_eq!(panes(&run.report()).len(), 3);
    let three = run.report();
    assert!(
        three["app"]["panes"].as_array().unwrap().iter().all(|pane| pane["pages_inside"] == true),
        "{}",
        three["app"]["panes"]
    );
    run.ok(&["qa", "compare-click", "1"]);
    run.settled();
    assert_eq!(panes(&run.report()).len(), 2);

    // Side by side inside a pane.
    run.ok(&["qa", "pane-focus", "1"]);
    run.ok(&["view", "--layout", "two-up"]);
    run.settled();
    let spreads = run.report();
    assert_eq!(panes(&spreads)[1]["spread"], "TwoUp");
    assert_eq!(panes(&spreads)[0]["spread"], "Single", "each tab keeps its own");
    assert!(spreads["app"]["panes"].as_array().unwrap().iter().all(|pane| pane["pages_inside"] == true));

    // A copy is the pane being read's: alpha's word, not gamma's.
    // A click in the pane, then Select All and Copy, which are the pane being read's.
    run.ok(&["qa", "mouse-down", &format!("{}:80,80:1", page_of(&spreads, 1))]);
    run.ok(&["qa", "mouse-up"]);
    run.ok(&["qa", "menu", "edit.select_all"]);
    run.ok(&["qa", "menu", "edit.copy"]);
    let began = Instant::now();
    let copied = loop {
        let copied = run.ok(&["qa", "copied"]);
        if copied["chars"].as_u64().unwrap_or(0) > 0 || began.elapsed() > Duration::from_secs(10) {
            break copied;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let text = copied["untrusted"]["text"].as_str().unwrap_or("");
    assert!(text.contains("alpha") && !text.contains("gamma"), "{copied}");

    // Closing the tab being read takes its pane, its page view, its thread and its worker.
    run.ok(&["close", "--tab", "#1"]);
    run.settled();
    let closed = run.report();
    assert_eq!(panes(&closed).len(), 1);
    assert!(closed["session"]["comparison"].is_null());
    assert_eq!(closed["session"]["active"], 3);
    // One-shot threads, a look at a file or a sleep file being written, end by themselves: a pane's page view that stayed would be there for good.
    let began = Instant::now();
    let threads = loop {
        let threads = run.report()["memory"]["threads"].as_u64().unwrap();
        if threads <= threads_alone + 1 || began.elapsed() > Duration::from_secs(5) {
            break threads;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(threads <= threads_alone + 1, "{threads} threads, {threads_alone} alone");

    // And a plain click returns to one.
    run.ok(&["compare", "--tabs", "#2,#3"]);
    run.settled();
    assert_eq!(panes(&run.report()).len(), 2);
    run.ok(&["qa", "click-row", "0"]);
    run.settled();
    let back = run.report();
    assert_eq!(panes(&back).len(), 1);
    assert!(back["app"]["panes"].as_array().unwrap()[0]["label"].is_null(), "one pane has no name on it");
}

/// Failure: a fourth PDF gets in, past what the live documents' rule allows, or it's refused with nothing said.
#[test]
fn a_fourth_pane_is_refused_with_words() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let files: Vec<TempPdf> = (0..4).map(|at| document(&format!("d{at}"), 3, (612.0, 792.0))).collect();
    for file in &files {
        run.ok(&["open", file.0.to_str().unwrap()]);
    }
    run.settled();
    run.ok(&["compare", "--tabs", "#1,#2,#3"]);
    run.settled();
    run.ok(&["qa", "compare-click", "3"]);
    run.settled();
    let report = run.report();
    assert_eq!(panes(&report).len(), 3);
    assert!(
        report["app"]["copy_notice"]["text"].as_str().is_some_and(|text| text.contains("three")),
        "{}",
        report["app"]["copy_notice"]
    );
}

/// Failure: a tab gone dormant, with its sleep file, comes into a comparison blank or by reading its PDF with the session held; a pane's file gone missing or replaced under it hangs the app or takes the other pane with it; or a pane's tab can be put to sleep while it's on screen.
#[test]
fn a_dormant_tab_and_a_missing_file_in_a_comparison() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let (a, b, c) = (
        document("alpha", 20, (612.0, 792.0)),
        document("beta", 8, (612.0, 792.0)),
        document("gamma", 5, (612.0, 792.0)),
    );
    for file in [&a, &b, &c] {
        run.ok(&["open", file.0.to_str().unwrap()]);
        run.settled();
    }
    let slept = run.ok(&["session", "sleep", "--tab", "#1"]);
    assert_eq!(slept["dormant"], true, "{slept}");
    let woken_before = run.report()["record"]["sleep"]["woken_from_file"].as_u64().unwrap_or(0);
    run.ok(&["qa", "compare-click", "0"]);
    run.settled();
    let compared = run.report();
    assert_eq!(panes(&compared).len(), 2);
    assert!(
        compared["record"]["sleep"]["woken_from_file"].as_u64().unwrap_or(0) > woken_before,
        "from its sleep file: {}",
        compared["record"]["sleep"]
    );
    assert!(panes(&compared).iter().all(|pane| pane["settled"] == true), "{}", compared["panes"]);
    assert_eq!(
        run.ok(&["session", "sleep", "--tab", "#3"])["dormant"],
        false,
        "a pane on screen doesn't go dormant"
    );

    // The other pane's file goes away and another comes in its place, under it.
    std::fs::remove_file(&c.0).unwrap();
    std::fs::write(&c.0, b"not a pdf at all").unwrap();
    run.ok(&["qa", "pane-scroll", "0:2000"]);
    run.ok(&["qa", "pane-focus", "0"]);
    run.settled();
    let after = run.report();
    assert_eq!(panes(&after).len(), 2, "the comparison stays");
    assert_eq!(after["hangs"]["over_2s"], 0);
    run.ok(&["qa", "pane-scroll", "1:900"]);
    run.settled();
    assert!(panes(&run.report())[1]["page"].as_i64().unwrap() > 1, "the pane whose file is fine still reads");
}

/// Failure (review 1): a comparison of cold tabs made by an agent leaves more live documents than SPEC §2's three: only the pane being read was made warm, and pane 0's page view kept every warm tab off screen beside the panes. Counted over every page view, and by workers.
#[test]
fn an_agent_comparison_of_cold_tabs_keeps_to_the_live_documents() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let files: Vec<TempPdf> = (0..5).map(|at| document(&format!("live{at}"), 6, (612.0, 792.0))).collect();
    for file in &files {
        run.ok(&["open", file.0.to_str().unwrap()]);
        run.settled();
    }
    // #5 showing, #4 and #3 warm, #1 and #2 cold.
    run.ok(&["compare", "--tabs", "#1,#2,#3"]);
    run.settled();
    let report = run.report();
    assert_eq!(panes(&report).len(), 3);
    let held = report["panes"]["documents_held"].as_u64().unwrap();
    assert!(held <= 3, "{held} documents held over the page views: {}", report["panes"]);
    assert!(report["memory"]["workers"].as_u64().unwrap() <= 3, "{} workers", report["memory"]["workers"]);
}

/// Failure (review 1): a comparison's pane that isn't being read, whose file has gone missing by the next launch, comes back blank with nothing said, as only the pane being read had a place for a note. It says so in the pane, under its name.
#[test]
fn a_missing_file_in_a_pane_says_so_after_a_restart() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let (a, b) = (document("gone", 6, (612.0, 792.0)), document("here", 6, (612.0, 792.0)));
    run.ok(&["open", a.0.to_str().unwrap()]);
    run.ok(&["open", b.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["compare", "--tabs", "#1,#2"]);
    run.settled();
    assert_eq!(run.report()["session"]["active"], 2, "the pane that stays is the one read");
    run.quit();
    std::fs::remove_file(&a.0).unwrap();
    let run = Run::on(&state).unwrap();
    let began = Instant::now();
    let report = loop {
        run.settled();
        let report = run.report();
        if !report["app"]["panes"][0]["note"].is_null() || began.elapsed() > Duration::from_secs(10) {
            break report;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(panes(&report).len(), 2, "the comparison comes back");
    let gone = &report["app"]["panes"][0];
    assert_eq!(gone["label"], "gone", "{gone}");
    assert!(gone["note"].as_str().is_some_and(|note| !note.is_empty()), "the pane says what's wrong: {gone}");
    assert!(report["app"]["panes"][1]["note"].is_null(), "the read pane's file is fine");
}

/// Failure (review 1): a right-click on a pane that isn't being read ticks the read pane's layout and changes the read pane's, as only `qa pane-focus` made a pane the one read. The right-click itself does, by what AppKit calls for one.
#[test]
fn a_right_click_on_a_pane_is_about_that_pane() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let (a, b) = (document("left", 8, (612.0, 792.0)), document("right", 8, (612.0, 792.0)));
    run.ok(&["open", a.0.to_str().unwrap()]);
    run.ok(&["open", b.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["compare", "--tabs", "#1,#2"]);
    run.settled();
    assert_eq!(run.report()["session"]["active"], 2);
    run.ok(&["qa", "pane-context-menu", "0:layout.two_up"]);
    run.settled();
    let after = run.report();
    assert_eq!(after["session"]["active"], 1, "the pane right-clicked is the one read");
    assert_eq!(panes(&after)[0]["spread"], "TwoUp", "and the layout chosen is its");
    assert_eq!(panes(&after)[1]["spread"], "Single", "and not the other's");
    run.ok(&["qa", "pane-focus", "1"]);
    run.settled();
    assert_eq!(run.report()["session"]["active"], 2, "pane-focus goes the same way");
}

/// Failure (found shooting review 1's pictures): restoring a comparison with a page smaller than its pane, a map fitted or a small card, took the reading pane and moved the others: the clip view centring that page at launch counted as the reader's scroll, which led synced scroll and focused the pane. Only the reader's hand leads, and everything comes back as it was.
#[test]
fn restoring_a_comparison_with_a_small_page_keeps_the_reader_and_the_places() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let (a, b, small) = (
        document("first", 30, (612.0, 792.0)),
        document("second", 30, (612.0, 792.0)),
        document("card", 1, (300.0, 200.0)),
    );
    for file in [&a, &b, &small] {
        run.ok(&["open", file.0.to_str().unwrap()]);
        run.settled();
    }
    run.ok(&["compare", "--tabs", "#1,#2,#3", "--sync", "on"]);
    run.settled();
    run.ok(&["qa", "pane-scroll", "0:5000"]);
    run.settled();
    let before = run.report();
    assert_eq!(before["session"]["active"], 1);
    let pages = |report: &Value| panes(report).iter().map(|pane| pane["page"].clone()).collect::<Vec<_>>();
    run.quit();
    let run = Run::on(&state).unwrap();
    run.settled();
    std::thread::sleep(Duration::from_millis(300));
    run.settled();
    let after = run.report();
    assert_eq!(after["session"]["active"], 1, "the pane read is the one that was");
    assert_eq!(after["panes"]["reading"], 0);
    assert_eq!(pages(&after), pages(&before), "each pane where it was");
}

/// The app's own objects and AppKit's views, scrollers, controls and gesture recognizers alive in the app's process, by class, as `heap` counts them. Nothing else: caches of strings, fonts and glyphs come and go with what's been drawn.
fn views_alive(pid: u32) -> std::collections::HashMap<String, u64> {
    let out = std::process::Command::new("/usr/bin/heap").arg(pid.to_string()).output().expect("heap runs");
    assert!(out.status.success(), "heap couldn't read the app: {}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    let mut alive = std::collections::HashMap::new();
    let mut in_table = false;
    for line in text.lines() {
        if line.trim_start().starts_with("=====") {
            in_table = true;
            continue;
        }
        if !in_table {
            continue;
        }
        // The first table is every zone together: the rest say it again by zone.
        if line.trim().is_empty() {
            break;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let Some(count) = fields.first().and_then(|count| count.parse::<u64>().ok()) else {
            continue;
        };
        if fields.len() < 6 {
            continue;
        }
        let (kind, binary) = (fields[fields.len() - 2], fields[fields.len() - 1]);
        let class = fields[3..fields.len() - 2].join(" ");
        let ours = binary == "RectoPDF" && matches!(kind, "Swift" | "ObjC");
        let views = binary == "AppKit" && kind == "ObjC" && ["View", "Scroller", "GestureRecognizer", "Control"].iter().any(|word| class.contains(word));
        if ours || views {
            *alive.entry(class).or_default() += count;
        }
    }
    assert!(!alive.is_empty(), "heap said nothing of the app's views:\n{text}");
    alive
}

/// Failure: a comparison leaves views behind when it ends, so a day of it grows for good. The 24-hour soak of 2026-11-08 grew 0.56 MiB an hour. Two ways were found, both views kept after they'd gone by an observer block AppKit gives each view (`-[NSView _commonAwake]`, in `leaks --traceTree`): the soak's build left an overlay scroller with its recognizers for each comparison, from the vertical scroller each pane had then; and the contents' rows, opening and closing as the pane read changes, left the row views the table animated, 0.6 a comparison with their cells (seven views each), until they faded over as a layer instead. Run with overlay scrollers and documents with contents. No class of the app's own or of AppKit's views may grow with every comparison, in two stretches running: something that moves once, as sidebar rows do, can't do that.
#[test]
fn comparisons_leave_no_views_behind() {
    const STRETCH: u64 = 12;
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let (left, right) = (common::app::chaptered("left", 12, 6), common::app::chaptered("right", 10, 5));
    run.ok(&["open", left.0.to_str().unwrap()]);
    run.ok(&["open", right.0.to_str().unwrap()]);
    run.settled();
    // Thin scrollers that fade, the style the app asks for unless the Mac always shows them.
    run.ok(&["qa", "scroller-style", "overlay"]);
    let compare = |times: u64| {
        for _ in 0..times {
            run.ok(&["compare", "--tabs", "#1,#2"]);
            run.settled();
            // A scroll shows the scrollers, which then fade.
            run.ok(&["qa", "scroll", "400"]);
            run.settled();
            run.ok(&["compare", "--off"]);
            run.settled();
        }
    };
    // What the first comparisons make once, AppKit's own included.
    compare(4);
    let first = views_alive(run.pid);
    compare(STRETCH);
    let second = views_alive(run.pid);
    compare(STRETCH);
    let third = views_alive(run.pid);
    let count = |of: &std::collections::HashMap<String, u64>, class: &str| of.get(class).copied().unwrap_or(0);
    let growing: Vec<String> = third
        .keys()
        .filter(|class| {
            let (a, b, c) = (count(&first, class), count(&second, class), count(&third, class));
            b >= a + STRETCH * 3 / 4 && c >= b + STRETCH * 3 / 4
        })
        .map(|class| format!("{class}: {} -> {} -> {}", count(&first, class), count(&second, class), count(&third, class)))
        .collect();
    assert!(growing.is_empty(), "grew with every comparison, {STRETCH} at a time: {growing:?}");
}
