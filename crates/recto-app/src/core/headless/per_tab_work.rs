//! **A person's action costs no more for the tabs it isn't about** (rule 1, and "a person's action may not wait"). Everything the session thread does for
//! one action, the action and the look that publishes after it, may walk the tabs a few times, as the sidebar must list them; it may not look each tab up
//! again by its number as it walks, which makes the action quadratic in the tabs open. On 2026-11-13 the sidebar did that for each tab's notice, and every
//! action at 1,000 tabs took about 0.5 ms more, which only the input-to-frame measurement (a release run, by hand) showed. Counted here instead, so any
//! machine and any build of the tests sees it: the records the session thread's searches for a tab looked at ([`recto_session::searched`], debug only).

use crate::core::controls::UiAction;
use crate::testkit::headless::{Disk, HeadlessApp, WAIT_LIMIT};

/// The records the session thread's searches looked at for one action: its call and the look after it, and nothing else. Drawing is held meanwhile, so no
/// page's word comes in; and what came before the call or after its look (a deadline, a word queued as it ran, the test's own flush) isn't counted, as each
/// is a call or a turn of its own. An action the session thread takes no call for (a zoom) is counted by the next call, the flush, and its look: what the
/// window's next look costs.
fn searched_for(app: &HeadlessApp, action: UiAction) -> u64 {
    app.settle();
    let engine = &app.core().engine;
    engine.hold_drawing(true);
    let seams = &app.core().serving.place_seams;
    seams.count_the_next_call();
    app.act(action);
    assert!(app.core().core.flush(WAIT_LIMIT), "the session thread took the call");
    let searched = seams.searched_by_the_counted_call().expect("the action's call and its look, counted");
    engine.hold_drawing(false);
    searched
}

/// The most records an action's call and the look after it may look at, a tab open: what the counts were on 2026-11-14 (31 a tab for a zoom's look to 74
/// for opening a file that's open already, at 400 tabs; 70 at 50), rounded up. Each is a few dozen lookups of the tab being read, as the action and the look
/// read its pane, its facts and its notices, and each lookup searches the tabs in order up to it: the last opened, so all of them here. A walk that looks
/// every tab up again adds half the tabs a tab for each lookup in it: the sidebar's notices made two, 50 records a tab at 50 tabs and 400 at 400. (Until
/// 2026-11-14 this was 55 for a window divided by its looks, the test's flush's look among them, which another call or a tab gone dormant moved.)
const SEARCHED_PER_TAB: u64 = 75;

/// Failure: an action, or the look that publishes after it, looks a tab up by its number for each tab it walks (the sidebar's notices did, until
/// 2026-11-13; and, with a password kept, the look's walk that keeps only the passwords of the files open did too), so its cost grows with the square of
/// the tabs open: at 1,000 tabs about a million records more for every action. Held two ways: no action
/// searches more than [`SEARCHED_PER_TAB`] records a tab in its call and the look after it, and none searches more a tab at 400 tabs than at 50, give or take a few lookups. 50 and 400
/// tabs, not 1,000: the same rule, seen in a third of the time.
#[test]
fn an_action_searches_the_tabs_no_more_a_tab_with_more_tabs() {
    let mut per_tab_at_50 = Vec::new();
    for tabs in [50_u64, 400] {
        let disk = Disk::new();
        let files: Vec<_> = (1..tabs).map(|at| disk.pdf(&format!("f{at:04}.pdf"), 3)).collect();
        let app = disk.start();
        // No tab goes dormant while it's counted: which are due goes by the session's real clock, not the kit's, and a tab gone dormant costs a look less, so
        // a slow round at 50 tabs read low against the one at 400 (34 records a tab against 23, load 74 to 92, 2026-11-14; with dormancy at a second, a
        // rotate's look went from 33 records a tab to 25 once two tabs had gone).
        app.core().qa_session_verb("dormant_after", "86400").expect("a QA verb").expect("set");
        // One protected file, opened with its password: what makes each look keep the passwords of the files open (a walk of its own).
        let locked = app.disk().file("locked.pdf", &super::files::locked_with("right", 1));
        app.act(crate::core::controls::UiAction::Open {
            path: locked.display().to_string(),
        });
        app.settle();
        let asked = app.reader().password_prompt.expect("the protected file asks for its password");
        app.core()
            .tools
            .prompts()
            .password_answered(asked.id, Some("right".into()), &app.core().core, &app.core().window);
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
            let searched = searched_for(&app, action);
            println!("{tabs} tabs, {name}: {searched} records searched");
            assert!(
                searched <= SEARCHED_PER_TAB * tabs,
                "{name} at {tabs} tabs searched {searched} records, over {SEARCHED_PER_TAB} a tab: something looks a tab up again for each tab it walks"
            );
            let per_tab = searched / tabs;
            match per_tab_at_50.get(at) {
                None => per_tab_at_50.push(per_tab),
                Some(&at_50) => assert!(
                    per_tab <= at_50 + 5,
                    "{name} searched {per_tab} records a tab at {tabs} tabs, {at_50} at 50: its cost grows with the tabs it isn't about"
                ),
            }
        }
        drop(app.quit());
    }
}
