//! **The update test of the slow stage** (`tools/check.sh`'s `slow` stage, docs/engineering/slow-mode.md): the hand-off of `an_update_in_the_middle_of_things_changes_nothing` with the lean setup of `slow_stage::make_rich_lean`, as the normal test's 25 settles cost 62 s of its 72 when every worker answer is late. Same state, same real zip, server, installer and swap, the same comparison.

use super::*;
use crate::slow_stage::make_rich_lean;

/// Failure (CI, 2026-11-11: an update's launch under slow conditions): an update loses or changes anything the user can see or has changed when the new app's launch is late at every waiting point: what it is known late, its state read 1.5 s late, the session thread late after it, the socket's first answer late (the relaunch carries the slow switches over), and its first frame's tiles drawn through workers whose answers are late. A launch an update opened waits for its state (`UPDATE_RESTORE_PATIENCE`, 20 s), so its window goes up with it: no window is made again here (`record.remade` is false), which is `everything_comes_back_when_the_state_is_read_late`'s. Crossed: the late state read inside an update's patience, a page on screen and settled before the hand-off and drawn again after it, the first correct frame, the whole of `qa state` compared, undo after it. Left the slow stage (R1.4): an update's launch with what it is and its state both late is held in process, each seam held and let through, by recto-app's `a_launch_an_update_opened_waits_for_its_state_however_late`.
#[test]
fn an_update_changes_nothing_when_every_wait_is_late() {
    let Some(bed) = Bed::new() else { return };
    let run = bed.start();
    let (a, b, c) = (long_document("alpha", 60), long_document("beta", 30), super::super::chaptered("gamma", 20, 8));
    run.ok(&["qa", "appearance", "dark"]);
    make_rich_lean(&run, &[&a, &b, &c]);
    run.settled();
    wait_for_journal(&run);
    let before = run.report();
    assert_eq!(before["session"]["tabs"].as_array().unwrap().len(), 3);
    assert!(before["session"]["tabs"][0]["edits"]["undo"].as_u64().unwrap() > 0);

    bed.release_of(NEW, Broken::Not);
    assert_eq!(run.ok(&["update", "check"])["status"], "available");
    assert_eq!(run.ok(&["update", "install"])["started"], true);
    let run = bed.wait_for_new_app(run);
    run.ok(&["qa", "scroller-style", "overlay"]);
    run.settled();
    let after = run.report();

    assert_eq!(bed.version(), NEW, "the new app is at the app's path");
    assert_eq!(bed.kept().as_deref(), Some(OLD), "the old one is kept");
    assert_same_but(&before, &after, &[], ACROSS_AN_UPDATE);
    for field in ["scroll_y", "scroll_x", "magnification"] {
        let (was, now) = (before["app"]["pages"][field].as_f64().unwrap(), after["app"]["pages"][field].as_f64().unwrap());
        assert!((was - now).abs() < 0.25, "{field}: {was} then {now}");
    }
    let shown = &after["record"]["first_shown"];
    assert_eq!(shown["home"], false, "{shown}");
    assert!(shown["tile_layers"].as_u64().unwrap() > 0, "something's on it: {shown}");
    assert_eq!(shown["page"], before["reader"]["page"]);
    assert_eq!(after["session"]["app"]["focus"]["view"], "find");
    assert_eq!(after["record"]["remade"], false, "an update's launch waited for its state");
    run.ok(&["qa", "owner-edit", "redo"]);
    run.quit();
}

/// The first tab's edits are journalled a little after they are made (fewer writes to the disk), which the long setup gave time for.
fn wait_for_journal(run: &Run<'_>) {
    crate::common::app::wait_for_long(run, |report| report["session"]["tabs"][0]["journal"].is_string());
}
