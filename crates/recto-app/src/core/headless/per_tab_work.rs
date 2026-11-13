//! **A person's action costs no more for the tabs it isn't about** (rule 1, and "a person's action may not wait"). Everything the session thread does for
//! one action, the action and the look that publishes after it, may walk the tabs a few times, as the sidebar must list them; it may not look each tab up
//! again by its number as it walks, which makes the action quadratic in the tabs open. On 2026-11-13 the sidebar did that for each tab's notice, and every
//! action at 1,000 tabs took about 0.5 ms more, which only the input-to-frame measurement (a release run, by hand) showed. Counted here instead, so any
//! machine and any build of the tests sees it: the records the session thread's searches for a tab looked at ([`recto_session::searched`], debug only).

use crate::core::controls::UiAction;
use crate::testkit::headless::{Disk, HeadlessApp, WAIT_LIMIT};

/// The records the session thread's searches looked at for one action, from the window's call to the end of the look after it, and the looks made: one,
/// unless something else asked for one meanwhile (a worker's answer, a timer), whose searches are in the count too.
fn searched_for(app: &HeadlessApp, action: UiAction) -> (u64, u64) {
    app.settle();
    let seams = &app.core().serving.place_seams;
    let (before, looks) = (seams.searched_at_look_end(), seams.looks_done());
    app.act(action);
    assert!(app.core().core.flush(WAIT_LIMIT), "the session thread took the call");
    (seams.searched_at_look_end() - before, (seams.looks_done() - looks).max(1))
}

/// The most records one look's searches may look at, a tab open: what the counts were after the fix (2026-11-13: 32 a look for a zoom or the sidebar
/// toggled, to 54 for opening a file that's open already, at 100 tabs and at 1,000 alike), rounded up. Each is a few dozen lookups of the tab being read,
/// as the action and the look read its pane, its facts and its notices, and each lookup searches the tabs in order up to it: the last opened, so all of them
/// here. A walk that looks every tab up again adds half the tabs a tab for each lookup in it: the sidebar's notices made two, 100 records a tab at 100
/// tabs and 1,000 at 1,000.
const SEARCHED_PER_TAB: u64 = 55;

/// Failure: an action, or the look that publishes after it, looks a tab up by its number for each tab it walks (the sidebar's notices did, until
/// 2026-11-13), so its cost grows with the square of the tabs open: at 1,000 tabs about a million records more for every action. Held two ways: no action
/// searches more than [`SEARCHED_PER_TAB`] records a tab a look, and none searches more a tab at 1,000 tabs than at 100, give or take a few lookups.
#[test]
#[cfg(debug_assertions)]
fn an_action_searches_the_tabs_no_more_a_tab_with_more_tabs() {
    let mut per_tab_at_100 = Vec::new();
    for tabs in [100_u64, 1000] {
        let disk = Disk::new();
        let files: Vec<_> = (0..tabs).map(|at| disk.pdf(&format!("f{at:04}.pdf"), 3)).collect();
        let app = disk.start();
        for file in &files {
            app.open(file);
        }
        let rows = app.tabs();
        assert_eq!(rows.len() as u64, tabs, "every file has its tab");
        let (a, b) = (rows[rows.len() - 1].clone(), rows[rows.len() - 2].clone());
        let actions = [
            ("tap row", UiAction::Row { row: b, adding: false }),
            ("tap row back", UiAction::Row { row: a, adding: false }),
            ("typed page", UiAction::PageTyped { text: "2".into() }),
            ("zoom", UiAction::Menu { id: "view.zoom_in".into() }),
            ("sidebar toggle", UiAction::Menu { id: "view.sidebar".into() }),
            ("rotate", UiAction::Menu { id: "rotate.right".into() }),
            ("undo", UiAction::Menu { id: "edit.undo".into() }),
            (
                "open its tab",
                UiAction::Open {
                    path: files[files.len() - 2].display().to_string(),
                },
            ),
        ];
        for (at, (name, action)) in actions.into_iter().enumerate() {
            let (searched, looks) = searched_for(&app, action);
            println!("{tabs} tabs, {name}: {searched} records searched, {looks} looks");
            let searched = searched / looks;
            assert!(
                searched <= SEARCHED_PER_TAB * tabs,
                "{name} at {tabs} tabs searched {searched} records, over {SEARCHED_PER_TAB} a tab: something looks a tab up again for each tab it walks"
            );
            let per_tab = searched / tabs;
            match per_tab_at_100.get(at) {
                None => per_tab_at_100.push(per_tab),
                Some(&at_100) => assert!(
                    per_tab <= at_100 + 5,
                    "{name} searched {per_tab} records a tab at {tabs} tabs, {at_100} at 100: its cost grows with the tabs it isn't about"
                ),
            }
        }
        drop(app.quit());
    }
}
