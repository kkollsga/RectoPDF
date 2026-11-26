//! `--ref` on `goto`, `select`, `copy`, `highlight`, `snapshot` and `export svg` through the app core, with no window: each reads the page a reference
//! points at now, as `text --ref` does, refuses what `text --ref` refuses in its words and does nothing then, and gives an agent no more than its level.
//! One executor each (`recto_agent::commands`); the six share `recto_agent::locating`'s resolver.

use super::changed_under_edits::{look, pdf_bytes, replace};
use super::files::locked_saying;
use super::locating::{chars_reference, drew, hostile_chars, owners_tab, pages_saying, reference, run_saying};
use super::withheld_events::{PASSWORD, person_unlocks};
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::{Level, exit};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// One command with a reference, as an agent sends it: its name, the lowest level that may run it, the line, and the file it writes, if it does.
struct Asked {
    name: &'static str,
    level: Level,
    line: Value,
    writes: Option<PathBuf>,
}

/// Every command that takes `--ref`, each given `token` for tab `#1`, with files for the writers named by `tag` in `dir`. `rect` is a box over the whole page.
fn asked(token: &str, dir: &Path, tag: &str) -> Vec<Asked> {
    let (png, svg) = (dir.join(format!("{tag}.png")), dir.join(format!("{tag}.svg")));
    let rect = json!([0, 0, 612, 792]);
    vec![
        Asked {
            name: "goto",
            level: Level::Control,
            line: json!({ "cmd": "goto", "ref": token, "tab": "#1" }),
            writes: None,
        },
        Asked {
            name: "select",
            level: Level::ReadOnly,
            line: json!({ "cmd": "select", "ref": token, "tab": "#1" }),
            writes: None,
        },
        Asked {
            name: "copy",
            level: Level::ReadOnly,
            line: json!({ "cmd": "copy", "ref": token, "tab": "#1" }),
            writes: None,
        },
        Asked {
            name: "highlight",
            level: Level::Control,
            line: json!({ "cmd": "highlight", "ref": token, "tab": "#1" }),
            writes: None,
        },
        Asked {
            name: "snapshot text",
            level: Level::ReadOnly,
            line: json!({ "cmd": "snapshot", "ref": token, "tab": "#1", "rect": rect, "as": "text" }),
            writes: None,
        },
        Asked {
            name: "snapshot png",
            level: Level::Full,
            line: json!({ "cmd": "snapshot", "ref": token, "tab": "#1", "rect": rect, "as": "png", "out": png }),
            writes: Some(png),
        },
        Asked {
            name: "export svg",
            level: Level::Full,
            line: json!({ "cmd": "export_svg", "ref": token, "tab": "#1", "out": svg }),
            writes: Some(svg),
        },
    ]
}

/// What the answer says of how the page was found, and that the token isn't anywhere in it.
fn found_as(name: &str, answer: &Value, token: &str, status: &str, was: u64) {
    assert_eq!(
        (answer["ref"]["status"].as_str(), answer["ref"]["was_page"].as_u64()),
        (Some(status), Some(was)),
        "{name}: {answer}"
    );
    assert!(!answer.to_string().contains(token), "{name}: the token isn't in the answer");
}

/// The words a command gave of the page's text, which for `select`, `copy` and a text snapshot is under `untrusted`.
fn words(answer: &Value) -> String {
    answer["untrusted"]["text"].as_str().unwrap_or_default().trim().to_owned()
}

/// Failure: the reason `--ref` is on these commands. Characters 0-4 of page 3 ("Mine") are referred to, and another program then puts a cover in front, so
/// the page is page 4. A command that goes by the number (or by the hint page in the reference) reads, goes to or highlights the wrong page. Each is at its
/// own level; found it's page 3 and moved it's page 4: `goto` lands on 4, `select` and `copy` give exactly "Mine", `highlight` marks page 4 only, a text
/// snapshot holds "Mine page 3", and `export svg` and a `snapshot` PNG write their files, and each says how the page was found.
#[test]
fn every_ref_command_reads_the_page_found_then_moved_after_another_programs_cover() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 5));
    app.open(&file);
    let token = chars_reference(&app, 3, 0, 4).expect("characters of page 3");
    let dir = file.parent().expect("a folder").to_path_buf();
    // Before: where the reference says. Highlight is left to after the cover: an unsaved edit would make another program's file a changed one.
    for each in asked(&token, &dir, "before").iter().filter(|each| each.name != "highlight") {
        let said = app
            .agent(Level::Full, &each.line.to_string())
            .unwrap_or_else(|error| panic!("{} found: {error:?}", each.name));
        found_as(each.name, &said, &token, "found", 3);
        match each.name {
            "goto" => assert_eq!(said["page"], 3, "{said}"),
            "select" | "copy" => assert_eq!(words(&said), "Mine", "{}: {said}", each.name),
            "snapshot text" => assert!(words(&said).contains("Mine page 3"), "{said}"),
            _ => {}
        }
        assert!(each.writes.as_ref().is_none_or(|out| out.exists()), "{} wrote its file", each.name);
    }
    replace(
        &file,
        &pages_saying(&["Cover", "Mine page 1", "Mine page 2", "Mine page 3", "Mine page 4", "Mine page 5"]),
    );
    look(&app);
    for each in asked(&token, &dir, "after") {
        let said = app
            .agent(Level::Full, &each.line.to_string())
            .unwrap_or_else(|error| panic!("{} moved: {error:?}", each.name));
        found_as(each.name, &said, &token, "moved", 3);
        match each.name {
            "goto" => assert_eq!(said["page"], 4, "{said}"),
            "select" | "copy" => assert_eq!(words(&said), "Mine", "{}: {said}", each.name),
            "highlight" => assert_eq!(said["highlighted"]["pages"], json!([4]), "{said}"),
            "snapshot text" => assert!(words(&said).contains("Mine page 3"), "{said}"),
            _ => {}
        }
        assert!(each.writes.as_ref().is_none_or(|out| out.exists()), "{} wrote its file", each.name);
    }
}

/// How each way of a reference not giving a page is made, on a fresh app with five pages whose page 3 is referred to by characters 0-4.
enum Case {
    Gone,
    Ambiguous,
    /// The owner deletes the page.
    Removed,
    /// The page is found, and its characters aren't the ones the reference was made of.
    TextChanged,
}

/// Failure: a reference to a page that isn't there, is on two pages, was deleted by the owner, or whose characters changed is read as something else by one
/// of the six commands (the nearest page, the first, the hint's page), goes there, highlights it, or draws it before refusing; or one command refuses in
/// words of its own so an agent can't tell which case it met; or the refusal carries the page's text. Each refuses `reference_unresolved`, exit not-found, in
/// the very words `text --ref` has for that case, and then nothing happened: no event of a page drawn or failed, the tab and its edits and the file as they
/// were, nothing written, and none of the page's words in what was said.
#[test]
fn every_ref_command_refuses_gone_ambiguous_removed_and_text_changed_in_its_own_words_and_does_nothing() {
    let mut said_by_case = Vec::new();
    for (case, tag) in [
        (Case::Gone, "gone"),
        (Case::Ambiguous, "ambiguous"),
        (Case::Removed, "removed"),
        (Case::TextChanged, "changed"),
    ] {
        let app = HeadlessApp::start();
        let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 5));
        app.open(&file);
        let good = chars_reference(&app, 3, 0, 4).expect("characters of page 3");
        let token = match case {
            Case::TextChanged => hostile_chars(&good, 0, 4),
            _ => good.clone(),
        };
        match case {
            Case::Gone => {
                replace(&file, &pages_saying(&["Mine page 1", "Mine page 2", "Mine page 4", "Mine page 5"]));
                look(&app);
            }
            Case::Ambiguous => {
                replace(&file, &pages_saying(&["Mine page 1", "Mine page 3", "Mine page 4", "Mine page 3"]));
                look(&app);
            }
            Case::Removed => {
                app.person(r##"{"cmd":"page_delete","tab":"#1","pages":"3"}"##);
                app.settle();
            }
            Case::TextChanged => {}
        }
        let dir = file.parent().expect("a folder").to_path_buf();
        let (as_text, _) = run_saying(&app, Level::ReadOnly, &json!({ "cmd": "text", "ref": token, "tab": "#1" }));
        let as_text = as_text.expect_err("text --ref refuses");
        assert_eq!(
            (as_text.code.as_str(), as_text.exit),
            ("reference_unresolved", exit::NOT_FOUND),
            "{tag}: {as_text:?}"
        );
        let before = (app.recto(Level::ReadOnly, &["where"]).expect("where"), owners_tab(&app, &file));
        for each in asked(&token, &dir, tag) {
            let (answer, events) = run_saying(&app, Level::Full, &each.line);
            let refused = answer.expect_err(&format!("{tag} {}", each.name));
            assert_eq!(
                (refused.code.as_str(), refused.exit, &refused.message),
                (as_text.code.as_str(), exit::NOT_FOUND, &as_text.message),
                "{tag} {}: the words of `text --ref`",
                each.name
            );
            assert!(!drew(&events), "{tag} {}: {events:?}", each.name);
            assert!(each.writes.as_ref().is_none_or(|out| !out.exists()), "{tag} {} left a file", each.name);
            assert!(
                !refused.message.contains("Mine") && !refused.message.contains(&token),
                "{tag} {}: {}",
                each.name,
                refused.message
            );
        }
        assert_eq!(
            (app.recto(Level::ReadOnly, &["where"]).expect("where"), owners_tab(&app, &file)),
            before,
            "{tag}: nothing of the tab, its edits or the file changed"
        );
        said_by_case.push(as_text.message);
    }
    let mut each_its_own = said_by_case.clone();
    each_its_own.sort();
    each_its_own.dedup();
    assert_eq!(each_its_own.len(), said_by_case.len(), "each case says its own thing: {said_by_case:?}");
}

/// How much an agent may do at a level, to compare two.
fn rank(level: Level) -> u8 {
    match level {
        Level::Off => 0,
        Level::ReadOnly => 1,
        Level::Control => 2,
        Level::Full => 3,
    }
}

/// The highlights of the tab as `highlights` lists them, with who made each.
fn highlights(app: &HeadlessApp) -> Vec<Value> {
    app.agent(Level::ReadOnly, r##"{"cmd":"highlights","tab":"#1"}"##).expect("highlights")["edits"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Failure: a reference is a way round a command's level, or round the owner's edits: below its level a command runs, writes a file, or an agent's highlight
/// undoes, replaces or reorders a step of the owner's. The owner turns page 9 and deletes page 2. At Off every one is refused for its level; at Read-only
/// `select`, `copy` and a text snapshot give their words and the rest are refused; at Control `goto` goes and `highlight` adds one step, by the agent, while
/// the file writers are refused and leave no file. The owner's steps, their undo history and the file are as they were at each.
#[test]
fn ref_commands_at_each_level_change_nothing_of_the_owners_edits() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 10));
    app.open(&file);
    app.person(r##"{"cmd":"page_rotate","tab":"#1","pages":"9","degrees":90}"##);
    app.person(r##"{"cmd":"page_delete","tab":"#1","pages":"2"}"##);
    app.settle();
    let token = chars_reference(&app, 4, 0, 4).expect("characters of the tab's page 4");
    let dir = file.parent().expect("a folder").to_path_buf();
    let before = owners_tab(&app, &file);
    assert_eq!(before.0["pages"], 9, "the owner's edits are on the tab: {}", before.0);
    assert!(highlights(&app).is_empty(), "and no highlight yet");
    for (level, tag) in [(Level::Off, "off"), (Level::ReadOnly, "readonly"), (Level::Control, "control")] {
        for each in asked(&token, &dir, tag) {
            let answer = app.agent(level, &each.line.to_string());
            if rank(level) >= rank(each.level) {
                let said = answer.unwrap_or_else(|error| panic!("{tag} {}: {error:?}", each.name));
                // The reference was made of the tab's page 4, which is the file's page 5 once the owner's delete counts.
                found_as(each.name, &said, &token, "found", 5);
            } else {
                let refused = answer.expect_err(&format!("{tag} {} is above its level", each.name));
                assert_eq!(refused.exit, exit::PERMISSION, "{tag} {}: {refused:?}", each.name);
            }
            assert!(
                each.writes.as_ref().is_none_or(|out| !out.exists()),
                "{tag} {} wrote a file below Full",
                each.name
            );
        }
        if level != Level::Control {
            assert_eq!(owners_tab(&app, &file), before, "{tag}: nothing of the owner's tab or file changed");
        }
    }
    let after = owners_tab(&app, &file);
    // `goto` moved the tab's place at Control, which is its job; the owner's pages and turns are theirs still.
    assert_eq!(
        (&after.0["runs"], &after.0["pages"]),
        (&before.0["runs"], &before.0["pages"]),
        "the owner's turn and delete are as they were"
    );
    assert_eq!((&after.2, after.3), (&before.2, before.3), "the file's bytes and time are as they were");
    let made = highlights(&app);
    assert_eq!(made.len(), 1, "Control's highlight is one: {made:?}");
    assert_eq!(made[0]["by"], "agent", "and it is the agent's own: {made:?}");
}

/// Failure: a document opened with its password gives an agent a page, or some of its words, through a reference, where `text` refuses it. Every command that
/// takes `--ref` is refused with `text`'s refusal for any valid token (one from an unprotected file, and a made-up one), and writes nothing.
#[test]
fn a_protected_document_refuses_every_ref_command_as_text_does() {
    let disk = Disk::new();
    let plain = disk.file("plain.pdf", &pages_saying(&["Locked page 1", "Locked page 2"]));
    let locked = disk.file("locked.pdf", &locked_saying(PASSWORD, 2, "Locked", lopdf::encryption::Permissions::all()));
    let app = disk.start();
    app.open(&plain);
    let twins = reference(&app, 2);
    let twins_chars = chars_reference(&app, 2, 0, 4).expect("characters");
    person_unlocks(&app, &locked);
    let tab = app.session_json()["tabs"]
        .as_array()
        .and_then(|tabs| tabs.iter().find(|tab| tab["path"].as_str().is_some_and(|path| path.ends_with("locked.pdf"))))
        .map(|tab| format!("#{}", tab["id"]))
        .expect("the locked tab");
    let text = app
        .agent(Level::Full, &json!({ "cmd": "text", "page": 2, "tab": tab }).to_string())
        .expect_err("text is refused");
    assert_eq!(text.exit, exit::PROTECTED);
    let dir = app.disk().files();
    for (token, tag) in [
        (twins, "page"),
        (twins_chars, "chars"),
        (recto_agent::commands::locating::EXAMPLE_REF.to_owned(), "made-up"),
        (recto_agent::commands::locating::EXAMPLE_REF_CHARS.to_owned(), "made-up-chars"),
    ] {
        for each in asked(&token, &dir, tag)
            .into_iter()
            .filter(|each| each.name != "highlight" || tag.ends_with("chars"))
        {
            let mut line = each.line.clone();
            line["tab"] = json!(tab);
            let answered = app.agent(Level::Full, &line.to_string()).expect_err("refused");
            assert_eq!(
                (answered.code, answered.exit, &answered.message),
                (text.code, text.exit, &text.message),
                "{tag} {}",
                each.name
            );
            assert!(each.writes.as_ref().is_none_or(|out| !out.exists()), "{tag} {} wrote a file", each.name);
        }
    }
}
