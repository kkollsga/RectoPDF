//! The owner's observations of 2026-11-08 about the reading view, in the real app: the page area's thumb, the margin at each end, text selection as the default tool, and Check for Updates in the RectoPDF menu.

use super::{Run, numbered};
use serde_json::{Value, json};

/// Failure: the page area keeps the system's scroller, with its wide track when the Mac shows classic ones, or its thumb isn't the sidebar's slim one, or it runs up under the agent icon, or takes width from the page. Owner, 2026-11-08: "Scroll bar track is quite wide. Can you make it thinner with a small margin on the right side so its centered on the thumb?", and 2026-11-07: "the scroll thumb always visible, the rail always hidden". By either system style, the thumb is `SCROLL_THUMB`'s width and inset, starts below the top right corner, moves as the reader scrolls, and the view's whole width is the page area's.
#[test]
fn the_page_area_has_the_slim_thumb_and_no_track() {
    let Some(run) = Run::start() else { return };
    let file = numbered(40);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let look = recto_session::SCROLL_THUMB;
    for style in ["legacy", "overlay"] {
        run.ok(&["qa", "scroller-style", style]);
        run.settled();
        let state = run.report();
        let pages = &state["app"]["pages"];
        assert_eq!(pages["vertical_scroller"], false, "no system scroller up and down, so no track: {pages}");
        let thumb = &pages["thumb"];
        assert_eq!(thumb["shown"], true, "{style}: {thumb}");
        assert!((thumb["thumb_width"].as_f64().unwrap() - look.width).abs() < 0.01, "{style}: {thumb}");
        assert!(
            (thumb["right_gap"].as_f64().unwrap() - look.inset).abs() < 0.01,
            "a small margin on the right: {thumb}"
        );
        assert!(
            thumb["thumb_top"].as_f64().unwrap() >= pages["scroller_top_inset"].as_f64().unwrap(),
            "below the zoom level and the agent icon: {pages}"
        );
        let pane = &state["app"]["panes"][0];
        assert!(
            (pages["view_width"].as_f64().unwrap() * pages["magnification"].as_f64().unwrap() - pane["width"].as_f64().unwrap()).abs() < 1.0,
            "{style}: nothing takes width from the page area: {pages} {pane}"
        );
    }
    let before = run.report()["app"]["pages"]["thumb"]["thumb_top"].as_f64().unwrap();
    run.ok(&["goto", "30"]);
    run.settled();
    let after = run.report()["app"]["pages"]["thumb"]["thumb_top"].as_f64().unwrap();
    assert!(after > before + 10.0, "it follows the place: {before} then {after}");
}

/// Failure: the first page can't be scrolled down to the middle of the window, or the last up to it, as the margin at each end isn't half a page; or opening a document starts half a page down. Owner, 2026-11-08: "The margin above first page, and below last page should be 50% of page height". The room past each end is what the layout says, and the view reaches it and no further.
#[test]
fn the_margin_at_each_end_is_half_a_page() {
    let Some(run) = Run::start() else { return };
    let file = numbered(6);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let pages = run.report()["app"]["pages"].clone();
    let (top, bottom) = (pages["end_room"][0].as_f64().unwrap(), pages["end_room"][1].as_f64().unwrap());
    assert!(top > 100.0 && bottom > 100.0, "half a letter page, less what's inside: {pages}");
    assert!(
        pages["scroll_y"].as_f64().unwrap().abs() < 1.0,
        "it opens at the first page, not half a page above it: {pages}"
    );
    run.ok(&["qa", "scroll", "-100000"]);
    let up = run.report()["app"]["pages"].clone();
    assert!(
        (up["scroll_y"].as_f64().unwrap() + top).abs() < 1.0,
        "as far up as the room and no further: {up}"
    );
    run.ok(&["qa", "scroll", "200000"]);
    let down = run.report()["app"]["pages"].clone();
    let furthest = down["document_height"].as_f64().unwrap() - down["view_height"].as_f64().unwrap() + bottom;
    assert!(
        (down["scroll_y"].as_f64().unwrap() - furthest).abs() < 1.0,
        "as far down as the room and no further: {down}"
    );
    assert_eq!(run.ok(&["where"])["page"], 6, "the last page is the place there");
}

fn snapshot_ticked(run: &Run<'_>) -> Value {
    let state = run.report();
    state["app"]["menu_bar"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|menu| menu["items"].as_array().unwrap().iter())
        .find(|item| item["id"] == "tools.snapshot")
        .map(|item| item["checked"].clone())
        .unwrap_or(Value::Null)
}

/// Failure: the snapshot tool is on at launch, or stays on in a document opened after it was turned on, so a drag draws a box instead of selecting text. Owner, 2026-11-08: "Text selection should be the default tool (not camera tool), enable camera tool via hotkey or sidebar menu". Off at launch, with the I-beam over text; on by Tools > Snapshot (what ⇧⌘A chooses) or the panel's button; off again when another tab comes to the front.
#[test]
fn text_selection_is_the_default_tool() {
    let Some(run) = Run::start() else { return };
    let (one, two) = (numbered(3), numbered(4));
    run.ok(&["open", one.0.to_str().unwrap()]);
    run.settled();
    assert_eq!(snapshot_ticked(&run), json!(false), "off at launch");
    // The answer says which tab the menu item acted on (owner, 2026-11-13: "Shouldnt it be clear which tab the action is sent to?").
    let chosen = run.ok(&["qa", "menu", "tools.snapshot", "--tab", "#1"]);
    assert_eq!(chosen["acted_on_tab"], "#1", "{chosen}");
    assert_eq!(snapshot_ticked(&run), json!(true), "on by its menu item and key");
    run.ok(&["open", two.0.to_str().unwrap()]);
    run.settled();
    assert_eq!(snapshot_ticked(&run), json!(false), "a new tab starts with text selection");
    run.ok(&["qa", "panel", "toggle"]);
    run.ok(&["qa", "panel", "tools.snapshot"]);
    assert_eq!(snapshot_ticked(&run), json!(true), "on by the panel's button");
    run.ok(&["qa", "click-row", "0"]);
    run.settled();
    assert_eq!(snapshot_ticked(&run), json!(false), "and off when another tab comes to the front");
}

/// Failure: Check for Updates… is missing from the RectoPDF menu, hidden or blank, in a build that isn't a signed release, as a development build or a quiet run's is. Owner, 2026-11-08: "Im not seeing check for updates button in the macos top bar menu". It's the RectoPDF menu's second item, with its words, and can be chosen.
#[test]
fn check_for_updates_is_in_the_rectopdf_menu_in_every_build() {
    let Some(run) = Run::start() else { return };
    let state = run.report();
    let first = &state["app"]["menu_bar"][0];
    assert_eq!(first["title"], "RectoPDF", "{first}");
    let item = &first["items"][1];
    assert_eq!(item["id"], "app.update", "{first}");
    assert!(
        item["title"]
            .as_str()
            .is_some_and(|title| title.starts_with("Check for Updates") || title.starts_with("Update to") || title.starts_with("Updating to")),
        "{item}"
    );
    assert_eq!(item["enabled"], true, "{item}");
    let status = run.ok(&["update", "status"]);
    assert!(status["build"].is_string(), "it says what kind of build this is: {status}");
}

/// A document whose pages differ in width: the first sets the fit, and the rest are narrower.
fn mixed_widths() -> recto_testpdf::TempPdf {
    use recto_testpdf::{Page, TempPdf, pages, text_lines};
    let list: Vec<Page> = [612.0_f32, 300.0, 500.0, 400.0, 612.0, 350.0]
        .iter()
        .map(|width| {
            let mut page = Page::new(text_lines(&["A page"]));
            page.media = (*width, 200.0);
            page
        })
        .collect();
    TempPdf::new("mixed.pdf", &pages(&list))
}

/// Failure (owner, 2026-11-12): "In some pdfs the pages aren't centered (with equal black gutters on both sides), pages area should always have equal black gutters on both sides to appear centered, with and without sidebar, and ui elements like zoom and page number shouldn't impact this." The right margin kept 96 points for the agent icon against the left's 56, so in a window narrower than about 950 points every page sat left of centre. In the real window, at a width where that showed, every page showing has the same room left and right (within a point), with the sidebar shown and hidden, whether the zoom level and the page number are showing or not, and for pages of different widths.
#[test]
fn pages_are_centred_in_the_page_area_with_the_sidebar_shown_and_hidden() {
    let Some(run) = Run::start() else { return };
    let file = mixed_widths();
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.ok(&["qa", "resize", "900x700"]);
    run.settled();
    let centred = |run: &Run<'_>, when: &str| {
        let state = run.report();
        let gutters = state["app"]["pages"]["page_gutters"].as_array().unwrap().clone();
        assert!(gutters.len() >= 2, "{when}: pages showing: {state}");
        for page in &gutters {
            let (left, right) = (page["left"].as_f64().unwrap(), page["right"].as_f64().unwrap());
            assert!(
                (left - right).abs() < 1.0,
                "{when}: page {} has {left} to its left and {right} to its right",
                page["page"]
            );
        }
    };
    centred(&run, "sidebar shown, level and number showing");
    run.ok(&["qa", "button", "sidebar"]);
    run.settled();
    assert_eq!(run.report()["app"]["sidebar_hidden"], true);
    centred(&run, "sidebar hidden");
    run.ok(&["qa", "button", "sidebar"]);
    run.settled();
    centred(&run, "sidebar back");
}
