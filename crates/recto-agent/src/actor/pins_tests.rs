//! **What each rule about who may do what answers today, for the person and each level** (the move of those rules onto [`Actor`], 2026-11-14): every
//! site of the owner's-edits rule, "a rewrite needs Full" and a file that asks "anyway", run as the person and as an agent at Read-only, Control and Full,
//! each on a fresh session over real files, its refusal's code, exit, words, next step and `ask` written to `pins.txt`. Moving a rule changes none of
//! them; a change that means to is made with `RECTO_BLESS=1`, and the file's diff is the review.

use crate::policy_guards::{every_command, fields, opened, pdf, restricted};
use crate::{Actor, Command, Level, execute};
use recto_testpdf::{Object, TempDir, TempPdf};
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
    /// One page of text, certified (a signature field whose DocMDP certifies it), so a copy of it is written anew.
    Certified,
}

/// One site: what the person did first, then the command each actor sends.
struct Case {
    name: &'static str,
    file: File,
    /// The person's steps, each a command's tag and the fields it's given.
    before: &'static [(&'static str, &'static str)],
    /// Steps the actor under test takes itself, after the person's and before `act`: its own edits, which it may lose where the owner's it may not. A
    /// refused step is left (an agent below Control can't take them), and `act` is then run as it is.
    own: &'static [(&'static str, &'static str)],
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
        own: &[],
        act: ("page_rotate", r#"{"pages":"2","degrees":90}"#),
    },
    Case {
        name: "an insert over the owner's undone step",
        file: File::Plain,
        before: &[TURN, UNDO],
        own: &[],
        act: ("page_insert", r#"{"path":"{other}","at":1}"#),
    },
    Case {
        name: "an undo of the owner's step",
        file: File::Plain,
        before: &[TURN],
        own: &[],
        act: UNDO,
    },
    Case {
        name: "a discard of the owner's edits",
        file: File::Plain,
        before: &[TURN],
        own: &[],
        act: ("discard", "{}"),
    },
    Case {
        name: "a clear-history of the owner's edits",
        file: File::Plain,
        before: &[TURN],
        own: &[],
        act: ("clear_history", "{}"),
    },
    Case {
        name: "a close over the owner's undone step",
        file: File::Plain,
        before: &[TURN, UNDO],
        own: &[],
        act: ("close", "{}"),
    },
    Case {
        name: "a close over the owner's unsaved edit",
        file: File::Plain,
        before: &[TURN],
        own: &[],
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
        own: &[],
        act: ("save", "{}"),
    },
    // A rewrite needs Full (SPEC section 5).
    Case {
        name: "a save that appends",
        file: File::Plain,
        before: &[TURN],
        own: &[],
        act: ("save", "{}"),
    },
    Case {
        name: "a save that removes a page",
        file: File::Plain,
        before: &[("page_delete", r#"{"pages":"1"}"#)],
        own: &[],
        act: ("save", "{}"),
    },
    Case {
        name: "a save-as over another file",
        file: File::Plain,
        before: &[TURN],
        own: &[],
        act: ("save", r#"{"out":"{there}","replace":true}"#),
    },
    // A file that asks not to be copied asks "anyway" (owner, 2026-11-07 and 08).
    Case {
        name: "a copy",
        file: File::Restricted,
        before: &[],
        own: &[],
        act: ("copy", r#"{"page":1,"chars":"0-3"}"#),
    },
    Case {
        name: "a select",
        file: File::Restricted,
        before: &[],
        own: &[],
        act: ("select", r#"{"page":1,"chars":"0-3"}"#),
    },
    Case {
        name: "an image",
        file: File::Restricted,
        before: &[],
        own: &[],
        act: ("image", r#"{"out":"{out}.png"}"#),
    },
    Case {
        name: "a snapshot",
        file: File::Restricted,
        before: &[],
        own: &[],
        act: ("snapshot", r#"{"out":"{out}.png","as":"png"}"#),
    },
    Case {
        name: "a render",
        file: File::Restricted,
        before: &[],
        own: &[],
        act: ("render", r#"{"out":"{out}.png"}"#),
    },
    Case {
        name: "an SVG export",
        file: File::Restricted,
        before: &[],
        own: &[],
        act: ("export_svg", r#"{"out":"{out}.svg"}"#),
    },
    Case {
        name: "a print to a PDF",
        file: File::Restricted,
        before: &[],
        own: &[],
        act: ("print", r#"{"to_pdf":"{out}.pdf"}"#),
    },
    // The states where a rule doesn't bite, so a change that makes it bite there fails too. SPEC line 121: at Control, an edit that clears undone steps
    // and a close with undone steps are allowed when every step lost is the agent's own.
    Case {
        name: "an edit over its own undone step",
        file: File::Plain,
        before: &[],
        own: &[TURN, UNDO],
        act: ("page_rotate", r#"{"pages":"2","degrees":90}"#),
    },
    Case {
        name: "a close over its own undone step",
        file: File::Plain,
        before: &[],
        own: &[TURN, UNDO],
        act: ("close", "{}"),
    },
    Case {
        name: "a redo of the owner's undone step",
        file: File::Plain,
        before: &[TURN, UNDO],
        own: &[],
        act: ("redo", "{}"),
    },
    // `--anyway` on a file that asks not to be copied passes, and a file that doesn't ask passes without it (OWNER-RULES 80, SPEC 162 and 164).
    Case {
        name: "a copy with --anyway",
        file: File::Restricted,
        before: &[],
        own: &[],
        act: ("copy", r#"{"page":1,"chars":"0-3","anyway":true}"#),
    },
    Case {
        name: "a copy of a file that isn't restricted",
        file: File::Plain,
        before: &[],
        own: &[],
        act: ("copy", r#"{"page":1,"chars":"0-3"}"#),
    },
    Case {
        name: "a snapshot with --anyway",
        file: File::Restricted,
        before: &[],
        own: &[],
        act: ("snapshot", r#"{"out":"{out}.png","as":"png","anyway":true}"#),
    },
    Case {
        name: "a snapshot of a file that isn't restricted",
        file: File::Plain,
        before: &[],
        own: &[],
        act: ("snapshot", r#"{"out":"{out}.png","as":"png"}"#),
    },
    Case {
        name: "a print to a PDF with --anyway",
        file: File::Restricted,
        before: &[],
        own: &[],
        act: ("print", r#"{"to_pdf":"{out}.pdf","anyway":true}"#),
    },
    Case {
        name: "a print to a PDF of a file that isn't restricted",
        file: File::Plain,
        before: &[],
        own: &[],
        act: ("print", r#"{"to_pdf":"{out}.pdf"}"#),
    },
    // What needs Full about a rewrite (SPEC line 122): one that takes out a highlight of the file's, and a copy of a certified file, which is written anew.
    Case {
        name: "a save that takes a highlight out of the file",
        file: File::Plain,
        before: &[
            ("highlight", r#"{"page":1,"chars":"0-3"}"#),
            ("save", "{}"),
            ("highlight_remove", r#"{"id":null,"page":1,"chars":"0-3"}"#),
        ],
        own: &[],
        act: ("save", "{}"),
    },
    Case {
        name: "a save-as copy of a certified file",
        file: File::Certified,
        before: &[TURN],
        own: &[],
        act: ("save", r#"{"out":"{out}.pdf"}"#),
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

/// One page of text, certified: a signature field whose value is a DocMDP signature, so a copy of it is written anew (SPEC decision 27).
fn certified() -> Vec<u8> {
    recto_testpdf::build(&[
        Object::Dict("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] /SigFlags 3 >> /Perms << /DocMDP 7 0 R >> >>".into()),
        Object::Dict("<< /Type /Pages /Kids [4 0 R] /Count 1 >>".into()),
        Object::Dict("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into()),
        Object::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>".into()),
        Object::Stream {
            dict: String::new(),
            data: recto_testpdf::text_lines(&["Page 1"]),
        },
        Object::Dict("<< /FT /Sig /T (Signature1) /V 7 0 R /Rect [0 0 0 0] /Subtype /Widget /P 4 0 R >>".into()),
        Object::Dict("<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 10 20 30] /Contents <00> /Reference [<< /TransformMethod /DocMDP /TransformParams << /P 2 >> >>] >>".into()),
    ])
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
        File::Certified => TempPdf::new("certified.pdf", &certified()),
    };
    let access = opened(&file.0);
    for (tag, set) in case.before {
        execute(&access, Actor::Person, &command(tag, set, &folder, &other.0), &mut Vec::new())
            .unwrap_or_else(|error| panic!("{}: {tag}: {error:?}", case.name));
    }
    for (tag, set) in case.own {
        // Refused below Control, and `act` is then asked as it stands.
        let _ = execute(&access, actor, &command(tag, set, &folder, &other.0), &mut Vec::new());
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
