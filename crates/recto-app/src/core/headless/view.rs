//! **How pages are shown, as the person changes it** (step R2.3, C5a): the paper, the sidebar, the agent icon, a tab's layout and the zoom, each the `view`
//! command an agent sends, run for the person.

use super::twins::{Twins, names};
use crate::core::controls::UiAction;
use crate::testkit::headless::Disk;

fn menu(id: &str) -> UiAction {
    UiAction::Menu { id: id.into() }
}

/// Failure (R2.3, accident A2): choosing the paper that's chosen already, or showing a sidebar that's showing, tells every watcher the view changed when
/// nothing did. Said once when it changes, and not again.
#[test]
fn a_view_change_is_announced_only_when_something_changed() {
    let app = Disk::new().start();
    app.hear_events();
    app.act(menu("paper.white"));
    app.settle();
    assert_eq!(names(&app.heard_events()), ["view_changed"], "white chosen: a change");
    app.act(menu("paper.white"));
    app.settle();
    let again = app.heard_events();
    assert!(
        !names(&again).contains(&"view_changed"),
        "white again: nothing changed, so nothing's said: {again:?}"
    );
}

/// Failure (R2.3, C5a): the window's paper, sidebar, agent icon, layout or zoom does something other than the `view` an agent sends: another change, another
/// event, or something else kept in the record. Each control, on one app, and its command at Full on another with the same documents: the same shown, said
/// and kept.
#[test]
fn the_windows_view_controls_are_the_agents_view() {
    let twins = Twins::on(&[("a.pdf", 6), ("b.pdf", 6)]);
    for (action, line) in [
        (menu("paper.dark"), r#"{"cmd":"view","paper":"dark"}"#),
        (menu("paper.warm"), r#"{"cmd":"view","paper":"warm"}"#),
        (UiAction::ToggleSidebar, r#"{"cmd":"view","sidebar":"toggle"}"#),
        (UiAction::ToggleSidebar, r#"{"cmd":"view","sidebar":"toggle"}"#),
        (menu("view.agent_icon"), r#"{"cmd":"view","agent_icon":"toggle"}"#),
        (menu("layout.two_up"), r#"{"cmd":"view","layout":"two-up"}"#),
        (menu("layout.two_up_cover"), r#"{"cmd":"view","layout":"two-up-cover"}"#),
        (menu("view.zoom_in"), r#"{"cmd":"view","zoom":"in"}"#),
        (
            UiAction::ZoomTyped {
                text: "150,5".into(),
                decimal: ",".into(),
            },
            r#"{"cmd":"view","zoom":"150.5%"}"#,
        ),
        (menu("view.fit"), r#"{"cmd":"view","zoom":"fit"}"#),
        (menu("view.actual_size"), r#"{"cmd":"view","zoom":"actual"}"#),
    ] {
        let what = format!("{action:?} and {line}");
        let before = twins.person.reader();
        let zoomed = twins.person.core().engine.magnification();
        let (person, agent) = twins.both(action, line);
        assert!(
            twins.person.reader() != before || twins.person.core().engine.magnification() != zoomed,
            "{what}: it changed something, so the comparison is of something"
        );
        assert_eq!(person, agent, "{what}: the same said");
        twins.assert_same(&what, &[]);
    }
}

/// Holds the session on another thread while `presses` runs here, as a command holding it for a moment would, so nothing the presses ask for is done, or
/// shown, until they've all been made: the person's quick presses, before the window's caught up.
fn while_the_session_is_busy(app: &crate::testkit::headless::HeadlessApp, presses: impl FnOnce()) {
    let core = std::sync::Arc::clone(app.core());
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let holder = std::thread::Builder::new()
        .name("test-holder".into())
        .spawn(move || {
            core.host.session.with_session(|_| {
                held_tx.send(()).expect("the test is waiting");
                let _ = done_rx.recv_timeout(crate::testkit::headless::WAIT_LIMIT);
            });
        })
        .expect("a thread");
    held_rx.recv_timeout(crate::testkit::headless::WAIT_LIMIT).expect("the session held");
    presses();
    drop(done_tx);
    holder.join().expect("the holder");
    app.settle();
}

/// Failure (R2.3, accident A3): the sidebar's button works out show or hide from what the window last showed, so a second press before the first is shown
/// asks for the same again and is lost: two quick presses leave it hidden. Each press toggles it as it is when it's done.
#[test]
fn two_quick_presses_of_the_sidebars_button_are_two_toggles() {
    let app = Disk::new().start();
    assert!(!app.reader().sidebar_hidden);
    while_the_session_is_busy(&app, || {
        app.act(UiAction::ToggleSidebar);
        app.act(UiAction::ToggleSidebar);
    });
    assert!(!app.reader().sidebar_hidden, "hidden and shown again");
    while_the_session_is_busy(&app, || {
        app.act(UiAction::ToggleSidebar);
    });
    assert!(app.reader().sidebar_hidden, "one press hides it");
}

/// Failure (R2.3, accident A4): a layout chosen on the page showing one tab lands on whichever tab is active by the time it's done, so a tap on another tab
/// just before it lays out that one instead. It names the tab in the pane it was chosen on.
#[test]
fn a_layout_lands_on_the_tab_it_was_chosen_on() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 4), disk.pdf("b.pdf", 4));
    let app = disk.start();
    app.open(&a);
    app.open(&b);
    let rows = app.tabs();
    let (first, second) = (rows[0].clone(), rows[1].clone());
    assert!(second.current, "b is showing");
    while_the_session_is_busy(&app, || {
        app.act(UiAction::Row {
            row: first.clone(),
            adding: false,
        });
        app.act(menu("layout.two_up"));
    });
    let layout = |tab: u32| {
        app.session_json()["tabs"]
            .as_array()
            .and_then(|tabs| tabs.iter().find(|kept| kept["id"] == tab))
            .map(|kept| kept["layout"].clone())
    };
    assert_eq!(layout(second.tab), Some("two-up".into()), "the tab it was chosen on");
    assert_eq!(layout(first.tab), Some("single".into()), "the tab tapped just before is left as it was");
}
