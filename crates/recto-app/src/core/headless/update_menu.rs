//! **The menu's update item and Settings' update choices, as the person's commands** (step R2.3, C8): what each sends, what follows, and when its answer is
//! put up as an alert, on the update bed, against the agent's line on an app of its own.

use crate::core::controls::UiAction;
use crate::core::updates::{AUTO_UPDATE_ROW, UPDATE_NOW_ROW};
use crate::testkit::headless::{Disk, HeadlessApp};
use crate::testkit::update_bed::{NEW, OLD, UpdateBed};
use recto_agent::Level;

/// An installed copy of [`OLD`] with a document open, and the bed it updates from, with [`NEW`] on offer if `offered`, and the offer seen.
fn running(offered: bool) -> (HeadlessApp, UpdateBed) {
    let disk = Disk::new();
    disk.install_app(OLD);
    let bed = UpdateBed::new();
    let file = disk.pdf("alpha.pdf", 3);
    let app = disk.builder().update(&bed).start();
    app.open(&file);
    if offered {
        bed.releases.offer(NEW);
        app.agent(Level::Full, r#"{"cmd":"update_check"}"#).expect("checked");
    }
    app.settle();
    (app, bed)
}

/// The person's commands a control sent, as their lines, settled.
fn sent(app: &HeadlessApp, act: impl FnOnce(&HeadlessApp)) -> Vec<String> {
    app.keep_sent();
    act(app);
    app.settle();
    app.sent().iter().map(recto_agent::Command::wire).collect()
}

fn menu(app: &HeadlessApp) {
    app.act(UiAction::Menu { id: "app.update".into() });
}

/// What the updater says of itself that two apps on two beds share.
fn status(app: &HeadlessApp) -> serde_json::Value {
    let report = app.update_status();
    serde_json::json!({ "status": report["status"], "version": report["version"], "menu": report["menu"] })
}

/// Whether the document window is shut, as the host says when it's closed.
fn window_shut(app: &HeadlessApp, shut: bool) {
    crate::lock(&app.core().window.app).window_closed = shut;
}

const INSTALL_NOW: &str = r#"{"cmd":"update_install","zip":null,"now":true}"#;
const CHECK: &str = r#"{"cmd":"update_check"}"#;

/// Failure (R2.3, C8): the menu's *Update to X…* does something other than an agent's `update install --now`, or checks instead of going. With the offer
/// seen, choosing it sends the person's `update install --now`, the app hands off for it, and the twin that an agent sent the line to ends the same.
#[test]
fn the_menus_update_with_an_offer_installs_as_the_agents_install_now() {
    let (person, bed) = running(true);
    assert_eq!(sent(&person, menu), [INSTALL_NOW]);
    let (agent, agent_bed) = running(true);
    agent.agent(Level::Full, INSTALL_NOW).expect("installing");
    agent.settle();
    // Each install goes on at its own pace from here; where it ends is the same.
    let (plan, _) = person.handed_off(&bed);
    let (agent_plan, _) = agent.handed_off(&agent_bed);
    assert_eq!((plan.to.as_str(), agent_plan.to.as_str()), (NEW, NEW));
}

/// Failure (R2.3, C8): the menu's *Check for Updates…* does something other than an agent's `update check`, or its answer isn't put up as an alert with no
/// document window to say it in, or is with one. Each sends the person's `update check` and ends as the agent's does; the answer is an alert only with the
/// window shut.
#[test]
fn the_menus_check_is_the_agents_check_and_alerts_only_with_no_window() {
    let (agent, _agent_bed) = running(false);
    agent.agent(Level::Full, CHECK).expect("checked");
    for shut in [true, false] {
        let (person, _bed) = running(false);
        window_shut(&person, shut);
        assert_eq!(sent(&person, menu), [CHECK]);
        assert_eq!(status(&person), status(&agent));
        let alert = person.core().take_update_alert();
        assert_eq!(alert.is_some(), shut, "window shut: {shut}, alert: {alert:?}");
    }
}

/// Failure (R2.3, C8): the menu's title said *Update to X…* and the offer had gone by the time the person's `update install --now` ran: it's refused, and
/// nothing happens, where the menu always checked then. The window follows the refusal with the person's `update check`, still the menu's, so its answer is
/// put up as an alert with the window shut. An agent's `install --now` is refused as before.
#[test]
fn the_menus_update_whose_offer_has_gone_checks() {
    let (app, _bed) = running(false);
    window_shut(&app, true);
    let lines = sent(&app, |app| {
        assert!(app.core().updates.menu().mark(), "the menu's step");
        app.person(INSTALL_NOW);
    });
    assert_eq!(lines, [INSTALL_NOW, CHECK]);
    assert!(app.core().take_update_alert().is_some(), "the check's answer as an alert");
    let refused = app.agent(Level::Full, INSTALL_NOW).expect_err("nothing on offer");
    assert_eq!(refused.code, "nothing_on_offer");
}

/// Failure (R2.3, C8): Settings' *Update Now* with its offer gone checks, or puts an alert up, as the menu does, where it did nothing; or a switch in
/// Settings isn't the agent's `settings`. The button sends `update install --now` and nothing follows or is alerted; *Update automatically* off is
/// `settings --auto-update off`, which the agent's twin ends the same as.
#[test]
fn settings_choices_are_the_agents_commands_and_the_button_alone_does_no_more() {
    let (app, _bed) = running(false);
    window_shut(&app, true);
    let lines = sent(&app, |app| app.core().setting_chosen(UPDATE_NOW_ROW, true));
    assert_eq!(lines, [INSTALL_NOW]);
    assert!(app.core().take_update_alert().is_none(), "nothing alerted");
    let lines = sent(&app, |app| app.core().setting_chosen(AUTO_UPDATE_ROW, false));
    assert_eq!(lines, [r#"{"cmd":"settings","auto_update":"off"}"#]);
    let (agent, _agent_bed) = running(false);
    agent.agent(Level::Full, r#"{"cmd":"settings","auto_update":"off"}"#).expect("set");
    agent.settle();
    assert_eq!(app.core().settings_model(), agent.core().settings_model());
}
