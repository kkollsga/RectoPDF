//! A protected file's password sheet: what it asks, how many tries, Cancel, and two asks at once. Twins of `recto-cli`'s app tests of the same names.

use super::files::locked_with;
use crate::testkit::headless::HeadlessApp;
use std::path::Path;

/// What the sheet over the window says now: its id and its words, or nothing.
fn sheet(app: &HeadlessApp) -> Option<(u64, String)> {
    app.reader().password_prompt.map(|prompt| (prompt.id, prompt.message))
}

/// The sheet's answer, as the host gives it, and what comes of it settled.
fn answer(app: &HeadlessApp, id: u64, password: Option<&str>) {
    app.core().password_answered(id, password.map(str::to_string));
    app.settle();
}

fn open(app: &HeadlessApp, path: &Path) {
    app.act(crate::core::controls::UiAction::Open {
        path: path.to_path_buf().display().to_string(),
    });
    app.settle();
}

/// Failure: wrong passwords are asked for without end, or Cancel leaves the sheet coming back at once; the person isn't told why it stopped. (The app test
/// `wrong_passwords_are_bounded_and_cancel_is_heard`, which this replaces.)
#[test]
fn wrong_passwords_are_bounded_and_cancel_is_heard() {
    let app = HeadlessApp::start();
    let file = app.disk().file("locked.pdf", &locked_with("right", 1));
    open(&app, &file);
    let (id, words) = sheet(&app).expect("a protected file asks for its password");
    assert!(words.contains("locked.pdf"), "the sheet names the file: {words}");
    answer(&app, id, None);
    assert_eq!(sheet(&app), None, "Cancel takes the sheet away, and it doesn't come back");
    assert!(app.reader().home, "nothing opened");
    open(&app, &file);
    for tries_left in (1..5).rev() {
        let (id, _) = sheet(&app).expect("asked again");
        answer(&app, id, Some("wrong"));
        let (_, words) = sheet(&app).unwrap_or_else(|| panic!("a wrong password asks again, with {tries_left} left"));
        assert!(words.contains(&format!("{tries_left} tr")), "with {tries_left} left it says so: {words}");
    }
    let (id, _) = sheet(&app).expect("the last try");
    answer(&app, id, Some("wrong"));
    assert_eq!(sheet(&app), None, "the last wrong try stops the asking");
    let notice = app.reader().selection_notice.map(|notice| notice.text).unwrap_or_default();
    assert!(notice.contains("stopped asking"), "and the person's told why: {notice:?}");
    assert!(app.reader().home, "nothing opened");
}

/// Failure (review O2): with two protected files opened together, the second's ask takes the first's place, or a password typed for the file the sheet names is
/// tried on the other, so neither opens, or the wrong one does. (The app test `two_protected_files_opened_together_each_get_their_own_password`, which this
/// replaces.)
#[test]
fn two_protected_files_opened_together_each_get_their_own_password() {
    let app = HeadlessApp::start();
    let first = app.disk().file("first.pdf", &locked_with("one", 1));
    let second = app.disk().file("second.pdf", &locked_with("two", 2));
    app.act(crate::core::controls::UiAction::Open {
        path: first.display().to_string(),
    });
    app.act(crate::core::controls::UiAction::Open {
        path: second.display().to_string(),
    });
    app.settle();
    let mut opened = Vec::new();
    for _ in 0..2 {
        let (id, words) = sheet(&app).expect("a sheet for each");
        // Whichever's asked about first, its own password opens it.
        let (password, name) = if words.contains("first.pdf") {
            ("one", "first")
        } else {
            assert!(words.contains("second.pdf"), "{words}");
            ("two", "second")
        };
        assert!(!opened.contains(&name), "asked about {name} twice: {words}");
        answer(&app, id, Some(password));
        let tabs: Vec<String> = app.tabs().into_iter().map(|row| row.text).collect();
        assert!(tabs.iter().any(|tab| tab == name), "its own password opens {name}: {tabs:?}");
        opened.push(name);
    }
    assert_eq!(sheet(&app), None);
    assert_eq!(app.tabs().len(), 2);
    let notice = app.reader().selection_notice.map(|notice| notice.text).unwrap_or_default();
    assert!(!notice.contains("didn't open"), "{notice}");
}

/// Failure (review O6): `qa password` types into the sheet for any file, one of the person's own included, rather than only a quiet run's test file in the
/// temp folders. Since 2026-11-13 a protected file outside them doesn't even open in a quiet run (the session's wall, `recto_session::confined`), so no
/// sheet asks for its password and the QA wall's refusal of an outside file's sheet is a second line nothing reaches; with no sheet the verb is refused, and a
/// test's own sheet is taken. (The app test `qa_password_is_only_for_a_tests_own_file` keeps only that the password never comes on the command line.)
#[test]
fn qa_password_types_only_into_a_tests_own_sheet() {
    let app = HeadlessApp::start();
    let refused = crate::core::qa::password_sheet_is_a_tests(app.core()).expect_err("no sheet");
    assert!(refused.message.contains("No password sheet"), "{refused:?}");
    let Some(outside) = crate::testkit::headless::outside_the_tests_folders("recto-qa-password.pdf") else {
        return;
    };
    std::fs::write(&outside, locked_with("open sesame", 1)).expect("a file outside the test's folders");
    open(&app, &outside);
    assert!(
        sheet(&app).is_none() && app.tabs().is_empty(),
        "a file outside the run's folders opened and asked for its password"
    );
    assert!(crate::core::qa::password_sheet_is_a_tests(app.core()).is_err(), "no sheet to type into");
    let own = app.disk().file("own-locked.pdf", &locked_with("open sesame", 1));
    open(&app, &own);
    assert!(sheet(&app).is_some());
    assert!(crate::core::qa::password_sheet_is_a_tests(app.core()).is_ok(), "a test's own file's sheet");
    let _ = std::fs::remove_file(&outside);
}
