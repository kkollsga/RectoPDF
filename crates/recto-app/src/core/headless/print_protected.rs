//! **The person's print of a document they opened with its password** (PLAN, "the person can't print a document they opened with its password"; the
//! design of 2026-11-14): it prints, through the person's own door, with their password for the tab's own file only, to a file with no name; and nothing
//! of it reaches an agent, or the server's route, which never carries the person's keys.

use super::files::locked_allowing;
use super::printing::{ALL, Panel, answer, hold_at, notice, print_files_left, printed};
use super::withheld_events::{person_unlocks, protected};
use crate::core::controls::UiAction;
use crate::testkit::headless::{Disk, WAIT_LIMIT};
use recto_agent::{Actor, Level};
use recto_control::host::Host;

/// **These tests are about the keys, not about how fast a starved worker opens:** every renderer of the test's process gets a page deadline it always
/// meets, as the comparison walks do. Under `--like-ci` a print's fresh worker was once killed at the 5 s deadline during its Open of a protected file
/// ("This file took too long to open.", 1 of 20, 2026-11-14): a product fault of its own, in PLAN.
pub(super) fn deadlines_met() {
    recto_render_service::page_deadline_for_this_process(std::time::Duration::from_secs(20));
}

/// What the person's print says when the document needs a password it wasn't given: the words `main` and 0.1.0 said for every print of such a document.
const NEEDS_PASSWORD: &str = "This PDF needs a password.";

/// **Failure (the bug, at v0.1.0 and on `main`): File > Print on a document the person opened with its password says "This PDF needs a password."**, as
/// the print's worker opened the file with no password, where the owner's rule is that Print doesn't refuse. All its pages, a range, and the selection's
/// pages each print, with nothing said.
#[test]
fn the_persons_print_of_a_password_opened_document_prints() {
    deadlines_met();
    let disk = Disk::new();
    let file = protected(&disk, 3, "Locked");
    let app = disk.start();
    person_unlocks(&app, &file);
    let mut lines = Vec::new();
    let mut print = |name: &str, panel: Panel| {
        answer(&app, panel);
        app.settle();
        let printed = printed(&app);
        app.settle();
        lines.push(format!("{name}: printed {printed} | after {}", notice(&app)));
    };
    print("all", ALL);
    print(
        "pages 2 to 3",
        Panel {
            all: false,
            first: 2,
            last: 3,
            ..ALL
        },
    );
    app.core().perform("edit.select_all".into());
    app.settle();
    print(
        "the selection",
        Panel {
            all: false,
            selection_only: true,
            ..ALL
        },
    );
    assert_eq!(
        lines,
        [
            "all: printed [612,612,612] | after -",
            "pages 2 to 3: printed [612,612] | after -",
            "the selection: printed [612,612,612] | after -",
        ],
        "\nnow:\n{}",
        lines.join("\n")
    );
}

/// **Failure (design point 5): a protected document that also asks not to be copied prints to a file with no "Print anyway".** The copy flag of a file
/// that opens only with its password couldn't be read without it, and the read answered "not restricted": it failed open, which was latent while the print
/// itself failed and is live once it has the key. To a file it asks, its "Print anyway" prints; on paper it never asks.
#[test]
fn a_protected_document_that_asks_not_to_be_copied_asks_print_anyway() {
    deadlines_met();
    let disk = Disk::new();
    let file = disk.file("protected.pdf", &locked_allowing("open sesame", 2, lopdf::encryption::Permissions::PRINTABLE));
    let app = disk.start();
    person_unlocks(&app, &file);
    answer(&app, Panel { to_file: true, ..ALL });
    app.settle();
    let refused = (printed(&app), notice(&app));
    let anyway = app.reader().selection_notice.and_then(|notice| notice.action_id).expect("Print anyway");
    app.act(UiAction::Menu { id: anyway });
    app.settle();
    let printed_anyway = printed(&app);
    app.settle();
    answer(&app, ALL);
    app.settle();
    let on_paper = (printed(&app), notice(&app));
    assert_eq!(
        refused,
        ("none".into(), "\"This file's permissions ask that it isn't copied.\" [Print anyway]".into()),
        "to a file it asks"
    );
    assert_eq!(printed_anyway, "[612,612]", "and Print anyway prints it");
    assert_eq!(on_paper, ("[612,612]".into(), "-".into()), "on paper it never asks");
}

/// **Failure (design point 2): closing the tab while its print is being prepared leaves the person's password held** by the print's renderer (and its
/// worker's copy) for as long as the print would have taken, or longer. Closed mid-way: the print stops at the next page, and nothing holds the key.
#[test]
fn closing_the_tab_mid_print_lets_the_key_go() {
    deadlines_met();
    let disk = Disk::new();
    let file = protected(&disk, 3, "Locked");
    let app = disk.start();
    person_unlocks(&app, &file);
    let key = std::sync::Arc::downgrade(&app.core().keys.for_path(&file).expect("the key typed for the tab is held"));
    let (held, go) = hold_at(1);
    answer(&app, ALL);
    held.recv_timeout(WAIT_LIMIT)
        .expect("the print held after its first page, with its renderer keyed");
    app.act(UiAction::Menu { id: "file.close".into() });
    app.wait_until("the tab closed", || app.tabs().is_empty());
    drop(go);
    app.settle();
    app.wait_until("nothing holds the key", || key.upgrade().is_none());
    assert_eq!(printed(&app), "none", "the print stopped");
    assert_eq!(print_files_left(), Vec::<String>::new());
}

/// **Failure (design point 1): a file put at a protected document's path after the person typed its password is opened with it**, as a key looked up by
/// path gives it: a tab with no edits checks nothing about its file before it prints, and one with edits checks a moment before it opens. The tab's file
/// replaced by another protected file (a new inode at the same path) is refused with the person's words, and nothing is printed.
#[test]
fn a_replaced_file_gets_no_key() {
    deadlines_met();
    let disk = Disk::new();
    let file = protected(&disk, 2, "Locked");
    let other = disk.file(
        "other.pdf",
        &super::files::locked_saying("open sesame", 2, "Other", lopdf::encryption::Permissions::all()),
    );
    let app = disk.start();
    person_unlocks(&app, &file);
    std::fs::rename(&other, &file).expect("another file put at the path");
    answer(&app, ALL);
    app.settle();
    assert_eq!(printed(&app), "none", "nothing printed of the file put there");
    assert!(notice(&app).contains(NEEDS_PASSWORD), "the person's words: {}", notice(&app));
}

/// **Failure (design point 1, and "an agent never gets the person's passwords"): a command the server runs gets the person's keys,** as the actor the
/// server names is only a word: `Host::run` with [`Actor::Person`], the trait the server holds, printing the protected tab to a file reads it with the
/// password. And an agent's `print` naming the protected tab runs, or is refused otherwise than it was before this branch; or one naming no file runs.
/// Through the server's route, the person's print of it needs a password and writes nothing; an agent's is refused before it runs, at Control by its
/// level, at Full as protected, nothing written; an agent's print with no file is refused with usage words.
#[test]
fn the_servers_route_and_an_agents_print_get_no_keys() {
    deadlines_met();
    let disk = Disk::new();
    let file = protected(&disk, 2, "Locked");
    let app = disk.start();
    person_unlocks(&app, &file);
    let out = app.disk().files().join("printed.pdf");
    let line = format!(r#"{{"cmd":"print","to_pdf":{}}}"#, serde_json::Value::from(out.display().to_string()));
    let command = recto_agent::parse(&line).expect("a print");
    let host: &dyn Host = &*app.core().host;
    let served = host.run(Actor::Person, &command, &mut Vec::new());
    assert_eq!(
        served.map(drop).map_err(|error| error.code),
        Err(recto_agent::Code::NEEDS_PASSWORD),
        "the server's route printed the protected tab with the person's key"
    );
    assert!(!out.exists(), "nothing written");
    let words = ["print", "--to-pdf", out.to_str().expect("a path")];
    let at_control = app.recto(Level::Control, &words).expect_err("refused at Control");
    let at_full = app.recto(Level::Full, &words).expect_err("refused at Full");
    assert_eq!(
        (at_full.code, at_full.exit),
        (recto_agent::Code::PROTECTED, recto_agent::exit::PROTECTED),
        "at Full: {at_full:?}"
    );
    assert_eq!(at_full.message, recto_agent::PROTECTED_NOTE);
    assert_eq!(at_control.exit, recto_agent::exit::PERMISSION, "at Control, by its level: {at_control:?}");
    assert!(!out.exists(), "nothing written");
    let plain = app.disk().pdf("plain.pdf", 1);
    app.open(&plain);
    app.settle();
    let unnamed = app.agent(Level::Full, r#"{"cmd":"print"}"#).expect_err("refused");
    assert_eq!(
        (unnamed.code, unnamed.exit),
        (recto_agent::Code::USAGE, recto_agent::exit::USAGE),
        "{unnamed:?}"
    );
    assert_eq!(printed(&app), "none", "nothing for the person's printer");
}
