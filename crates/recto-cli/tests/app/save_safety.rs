//! Save's safety in the real app: the agreement gate as the window shows it. The rule and its words are `recto_core::agreement`'s, and the look is
//! `recto_agent::agreement`'s, each unit-tested there; these check the window says what they decide.

use super::{Run, menu_item, wait_for};
use recto_testpdf::{Object, TempPdf, build};
use serde_json::json;

/// A file lopdf and PDFium walk differently: both count two pages, not the same first one.
fn walked_differently() -> TempPdf {
    TempPdf::new(
        "damaged.pdf",
        &build(&[
            Object::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
            Object::Dict("<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".into()),
            Object::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Kids [4 0 R] >>".into()),
            Object::Dict("<< /Type /Page /Parent 3 0 R /MediaBox [0 0 400 400] >>".into()),
            Object::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] >>".into()),
        ]),
    )
}

/// Failure (SPEC section 3, the agreement gate): the window offers Save on a file its two readers see differently, so the person's turn is written
/// onto another page than the one they saw, or the close prompt offers a Save that's refused. Once the look is done, the Save icon is off and says
/// why, ⌘S and Save As are greyed, a click does nothing, the close prompt offers only Cancel and Don't Save, and the file isn't touched.
#[test]
fn a_file_the_two_readers_see_differently_turns_save_off_in_the_window() {
    let Some(run) = Run::start() else { return };
    let file = walked_differently();
    let original = std::fs::read(&file.0).unwrap();
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "owner-edit", "rotate:1"]);
    let off = wait_for(&run, |state| {
        state["app"]["save_button"]["visible"] == true && state["app"]["save_button"]["enabled"] == false
    });
    let tooltip = off["app"]["save_button"]["tooltip"].as_str().unwrap();
    assert!(tooltip.contains("two readers see differently") && tooltip.contains("page 1"), "{tooltip}");
    assert_eq!(menu_item(&off, "file.save")["enabled"], false, "⌘S is off too");
    assert_eq!(menu_item(&off, "file.save_as")["enabled"], false, "and Save As (review round 1)");
    assert_eq!(menu_item(&off, "file.close")["enabled"], true, "Close isn't");
    let (refused, _) = run.state.json(&["qa", "button", "save"]);
    assert_eq!(refused["ok"], false, "a click on an icon that's off does nothing: {refused}");
    run.ok(&["qa", "menu", "file.close"]);
    let asked = wait_for(&run, |state| state["app"]["sheet"]["kind"] == "close_prompt");
    assert_eq!(asked["app"]["sheet"]["buttons"], json!(["Cancel", "Don't Save"]));
    assert!(
        asked["app"]["sheet"]["informative"].as_str().unwrap().contains("two readers see differently"),
        "{}",
        asked["app"]["sheet"]
    );
    run.ok(&["qa", "sheet-answer", "first"]);
    wait_for(&run, |state| state["app"]["sheet"].as_object().is_none_or(|sheet| sheet.is_empty()));
    assert_eq!(std::fs::read(&file.0).unwrap(), original, "not a byte changed");
}
