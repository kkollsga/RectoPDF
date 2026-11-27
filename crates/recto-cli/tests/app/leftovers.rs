//! **Clear Leftovers in the real window** (D6), as a step of a launch that is there already (`the_windows_save_close_prompt_and_save_as_act_on_their_own_tab`):
//! Edit > Clear Leftovers… with a crashed save beside the open document draws Rust's sheet over the window, shot light and dark, and its first button moves
//! the save to the Trash. What Rust decides is the headless tests' (`leftovers_sheet.rs`); this is only what the real window shows and does with it.

use super::common::app::{Run, numbered, wait_for, wait_for_within};
use super::sheet_of;
use serde_json::{Value, json};
use sha2::Digest;
use std::path::Path;

/// A crashed save of a process that is gone, beside `document`. Named as the writer names it (`recto_core::save::writer_temp_prefix`: `.append-`, the first
/// 16 hex digits of the file name's SHA-256, then the process and a counter, then `.tmp`).
fn crashed_save_beside(document: &Path) -> std::path::PathBuf {
    let digest = sha2::Sha256::digest(document.file_name().unwrap().as_encoded_bytes());
    let prefix: String = digest[..8].iter().map(|byte| format!("{byte:02x}")).collect();
    let temp = document.with_file_name(format!(".append-{prefix}-4000001-1.tmp"));
    std::fs::write(&temp, b"a crashed save").unwrap();
    temp
}

/// The sheet over the window, as `qa state` has it.
fn sheet(state: &Value) -> &Value {
    &state["app"]["sheet"]
}

/// Edit > Clear Leftovers… offers, shows light and dark, and the first button moves the crashed save to the Trash.
pub fn offers_and_clears(run: &Run<'_>) {
    let file = numbered(2);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let temp = crashed_save_beside(&file.0);
    // A clear needs the app to keep its record and the sleep pass to be over; a look doesn't, and the sheet's words say what it finds.
    wait_for(run, |_| run.ok(&["qa", "removal-state", "show"])["ready"] == true);
    run.ok(&["qa", "menu", "edit.clear_leftovers"]);
    let offered = sheet_of(run, "clear_leftovers");
    let buttons = sheet(&offered)["buttons"].clone();
    assert_eq!(buttons, json!(["Move to Trash and Delete", "Cancel"]), "{}", sheet(&offered));
    let name = file.0.file_name().unwrap().to_str().unwrap();
    assert!(
        sheet(&offered)["informative"].as_str().unwrap().contains(name),
        "the sheet names the document: {}",
        sheet(&offered)
    );
    for look in ["light", "dark"] {
        run.ok(&["qa", "appearance", look]);
        run.settled();
        let out = run.state.root.join(format!("leftovers-offer-{look}.png"));
        run.ok(&["qa", "shot", "--out", out.to_str().unwrap()]);
        if let Some(keep) = std::env::var_os("RECTO_TEST_SHOTS") {
            std::fs::create_dir_all(&keep).ok();
            std::fs::copy(&out, Path::new(&keep).join(out.file_name().unwrap())).ok();
        }
    }
    run.ok(&["qa", "appearance", "light"]);
    run.ok(&["qa", "sheet-answer", "first"]);
    let trash = run.state.root.join("trash-stand-in");
    wait_for_within(run, super::common::app::save_within(), |state| {
        !temp.exists() && trash.read_dir().is_ok_and(|mut moved| moved.next().is_some()) && sheet(state).as_object().is_none_or(|sheet| sheet.is_empty())
    });
}
