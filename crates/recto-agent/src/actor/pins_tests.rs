//! **What each rule about who may do what answers today, for the person and each level** (the move of those rules onto [`Actor`], 2026-11-14): every
//! site of the owner's-edits rule, "a rewrite needs Full" and a file that asks "anyway", run as the person and as an agent at Read-only, Control and Full,
//! each on a fresh session over real files, its refusal's code, exit, words, next step and `ask` written to `pins.txt`. Moving a rule changes none of
//! them; a change that means to is made with `RECTO_BLESS=1`, and the file's diff is the review.

use crate::policy_guards::{every_command, fields, opened, pdf, restricted};
use crate::{Actor, Command, Level, execute};
use recto_testpdf::{TempDir, TempPdf};
use serde_json::{Map, Value, json};
use std::path::Path;

const PINS: &str = include_str!("pins.txt");

const ACTORS: [Actor; 4] = [
    Actor::Person,
    Actor::Agent(Level::ReadOnly),
    Actor::Agent(Level::Control),
    Actor::Agent(Level::Full),
];

/// The document a case runs on.
#[derive(Clone, Copy)]
enum File {
    /// Four pages of text, each with its number.
    Plain,
    /// Two pages, whose permissions ask that it isn't copied.
    Restricted,
}

/// One site: what the person did first, then the command each actor sends.
struct Case {
    name: &'static str,
    file: File,
    /// The person's steps, each a command's tag and the fields it's given.
    before: &'static [(&'static str, &'static str)],
    /// The command each actor sends.
    act: (&'static str, &'static str),
}

const TURN: (&str, &str) = ("page_rotate", r#"{"pages":"1","degrees":90}"#);
const UNDO: (&str, &str) = ("undo", "{}");

const CASES: &[Case] = &[
    // The owner's edits (SPEC decision 20).
    Case {
        name: "an edit over the owner's undone step",
        file: File::Plain,
        before: &[TURN, UNDO],
        act: ("page_rotate", r#"{"pages":"2","degrees":90}"#),
    },
    Case {
        name: "an insert over the owner's undone step",
        file: File::Plain,
        before: &[TURN, UNDO],
        act: ("page_insert", r#"{"path":"{other}","at":1}"#),
    },
    Case {
        name: "an undo of the owner's step",
        file: File::Plain,
        before: &[TURN],
        act: UNDO,
    },
    Case {
        name: "a discard of the owner's edits",
        file: File::Plain,
        before: &[TURN],
        act: ("discard", "{}"),
    },
    Case {
        name: "a clear-history of the owner's edits",
        file: File::Plain,
        before: &[TURN],
        act: ("clear_history", "{}"),
    },
    Case {
        name: "a close over the owner's undone step",
        file: File::Plain,
        before: &[TURN, UNDO],
        act: ("close", "{}"),
    },
    Case {
        name: "a close over the owner's unsaved edit",
        file: File::Plain,
        before: &[TURN],
        act: ("close", "{}"),
    },
    Case {
        name: "a save that drops the owner's steps",
        file: File::Plain,
        before: &[
            ("highlight", r#"{"page":4,"chars":"0-3"}"#),
            ("save", "{}"),
            ("page_delete", r#"{"pages":"4"}"#),
        ],
        act: ("save", "{}"),
    },
    // A rewrite needs Full (SPEC section 5).
    Case {
        name: "a save that appends",
        file: File::Plain,
        before: &[TURN],
        act: ("save", "{}"),
    },
    Case {
        name: "a save that removes a page",
        file: File::Plain,
        before: &[("page_delete", r#"{"pages":"1"}"#)],
        act: ("save", "{}"),
    },
    Case {
        name: "a save-as over another file",
        file: File::Plain,
        before: &[TURN],
        act: ("save", r#"{"out":"{there}","replace":true}"#),
    },
    // A file that asks not to be copied asks "anyway" (owner, 2026-11-07 and 08).
    Case {
        name: "a copy",
        file: File::Restricted,
        before: &[],
        act: ("copy", r#"{"page":1,"chars":"0-3"}"#),
    },
    Case {
        name: "a select",
        file: File::Restricted,
        before: &[],
        act: ("select", r#"{"page":1,"chars":"0-3"}"#),
    },
    Case {
        name: "an image",
        file: File::Restricted,
        before: &[],
        act: ("image", r#"{"out":"{out}.png"}"#),
    },
    Case {
        name: "a snapshot",
        file: File::Restricted,
        before: &[],
        act: ("snapshot", r#"{"out":"{out}.png","as":"png"}"#),
    },
    Case {
        name: "a render",
        file: File::Restricted,
        before: &[],
        act: ("render", r#"{"out":"{out}.png"}"#),
    },
    Case {
        name: "an SVG export",
        file: File::Restricted,
        before: &[],
        act: ("export_svg", r#"{"out":"{out}.svg"}"#),
    },
    Case {
        name: "a print to a PDF",
        file: File::Restricted,
        before: &[],
        act: ("print", r#"{"to_pdf":"{out}.pdf"}"#),
    },
];

/// The command `tag` as [`every_command`] has it, on tab `#1`, with `set`'s fields over its own: the placeholders `{other}` (a PDF to read),
/// `{there}` (a file that's there) and `{out}` (a name nothing has) filled in.
fn command(tag: &str, set: &str, folder: &Path, other: &Path) -> Command {
    let set = set
        .replace("{other}", other.to_str().unwrap())
        .replace("{there}", folder.join("there.pdf").to_str().unwrap())
        .replace("{out}", folder.join(format!("out-{tag}")).to_str().unwrap());
    let base = every_command()
        .into_iter()
        .find(|command| command.tag() == tag)
        .unwrap_or_else(|| panic!("{tag} in every_command"));
    let mut asked: Map<String, Value> = fields(&base);
    let Ok(Value::Object(set)) = serde_json::from_str::<Value>(&set) else {
        panic!("{set}")
    };
    asked.extend(set);
    asked.insert("tab".into(), json!("#1"));
    crate::parse(&Value::Object(asked).to_string()).unwrap_or_else(|error| panic!("{tag}: {error:?}"))
}

/// What a case answered `actor`, on one line: a refusal's code, exit, words, next step and ask; or `ok`, with whether a save says where its backup is
/// and a selection's note.
fn answered(case: &Case, actor: Actor) -> String {
    let folder = TempDir::new("recto-actor-pins-");
    let other = TempPdf::new("other.pdf", &pdf(2));
    std::fs::write(folder.join("there.pdf"), pdf(1)).unwrap();
    let file = match case.file {
        File::Plain => TempPdf::new("owners.pdf", &pdf(4)),
        File::Restricted => TempPdf::new("restricted.pdf", &restricted()),
    };
    let access = opened(&file.0);
    for (tag, set) in case.before {
        execute(&access, Actor::Person, &command(tag, set, &folder, &other.0), &mut Vec::new())
            .unwrap_or_else(|error| panic!("{}: {tag}: {error:?}", case.name));
    }
    let (tag, set) = case.act;
    let said = match execute(&access, actor, &command(tag, set, &folder, &other.0), &mut Vec::new()) {
        Ok(answer) => format!("ok, backup said: {}, note: {}", answer.get("backup").is_some(), answer["note"]),
        Err(error) => format!(
            "{} exit {} | {} | next: {} | ask: {:?}",
            error.code, error.exit, error.message, error.next, error.ask
        ),
    };
    format!("{} | {actor:?} | {said}", case.name)
}

/// Failure: moving a rule about who may do what (the owner's edits, a rewrite at Full, a copy "anyway") changes what one of its sites answers the
/// person or an agent at some level: a refusal's code, exit, words, next step or `ask`, or a refusal where there was none, or none where there was one.
#[test]
fn each_rule_answers_each_actor_as_it_did() {
    let lines: Vec<String> = CASES.iter().flat_map(|case| ACTORS.map(|actor| answered(case, actor))).collect();
    let now = lines.join("\n") + "\n";
    if std::env::var_os("RECTO_BLESS").is_some() {
        std::fs::write(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/actor/pins.txt"), &now).unwrap();
        return;
    }
    let differs: Vec<String> = PINS
        .lines()
        .zip(now.lines())
        .filter(|(was, is)| was != is)
        .map(|(was, is)| format!("was {was}\nnow {is}"))
        .collect();
    assert!(
        differs.is_empty() && PINS.lines().count() == now.lines().count(),
        "the rules answer otherwise:\n{}\n\n{now}",
        differs.join("\n\n")
    );
}
