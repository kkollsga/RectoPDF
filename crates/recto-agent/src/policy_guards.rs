//! **Each policy a command meets, walked over every command** (R4.2, `docs/FEATURES.md`): what an agent may see of a document the person opened with its
//! password, that an agent below Full loses none of the owner's edits, that a file a command writes never goes over one that's there, and that a file
//! which asks not to be copied asks "anyway" of every command that copies or exports it. Each is decided where it's decided (the gate in
//! [`crate::execute`], the history's rules, `render`'s publishing, `extract`'s refusal); these walk [`every_command`], so a command added later is held by
//! them without a word here. The levels' own walk is `walking_every_command_at_every_level` in `lib.rs`, and the quiet run's file wall's is in
//! `quiet_files`.

use crate::{Actor, Command, Condition, Level, commands, execute};
use recto_session::Session;
use recto_testpdf::{Page, TempPdf, pages, text_lines};
use serde_json::Value;
use std::path::Path;
use std::sync::Mutex;

/// One of every command the session answers, each with its tab `#1`: the list the walks here and in `lib.rs` go over.
/// `every_command_has_a_row_and_every_session_row_has_a_command` holds it to the table, so a session command added later must be added here.
pub(crate) fn every_command() -> Vec<Command> {
    let tab = || crate::TabRef::from("#1");
    vec![
        Command::from(crate::commands::tabs::Open {
            path: "/d/a.pdf".into(),
            page: None,
        }),
        Command::from(crate::commands::reading::Tabs { from: None, limit: None }),
        Command::from(crate::commands::reading::Sidebar {}),
        Command::from(crate::commands::tabs::Tap { tab: tab() }),
        Command::from(crate::commands::linking::Goto {
            tab: tab(),
            page: Some(1),
            label: None,
            within: None,
        }),
        Command::from(crate::commands::linking::Links { tab: tab(), page: 1 }),
        Command::from(crate::commands::linking::Follow { tab: tab(), page: 1, link: 1 }),
        Command::from(crate::commands::linking::Back { tab: tab() }),
        Command::from(crate::commands::linking::Forward { tab: tab() }),
        Command::from(crate::commands::tabs::Recent {}),
        Command::from(crate::commands::tabs::RecentClear {}),
        Command::from(crate::commands::tabs::Close { tab: tab() }),
        Command::from(crate::commands::measure::Budget {}),
        Command::from(crate::commands::compare::Compare::Look),
        Command::from(crate::commands::compare::Compare::Off),
        Command::from(crate::commands::export::Render {
            tab: tab(),
            page: 1,
            scale: 2.0,
            out: "/d/x.png".into(),
            anyway: false,
        }),
        Command::from(crate::commands::export::ExportSvg {
            tab: tab(),
            page: 1,
            out: "/d/x.svg".into(),
            rect: None,
            text: None,
            scale: 2.0,
            strip_metadata: false,
            anyway: false,
        }),
        Command::from(crate::commands::export::Images { tab: tab(), page: 1 }),
        Command::from(crate::commands::export::Image {
            tab: tab(),
            page: 1,
            number: 1,
            out: "/d/x.png".into(),
            anyway: false,
        }),
        Command::from(crate::commands::export::Snapshot {
            tab: tab(),
            page: 1,
            rect: [0.0, 0.0, 10.0, 10.0],
            format: "png".into(),
            out: Some("/d/x.png".into()),
            scale: 2.0,
            anyway: false,
        }),
        Command::from(crate::commands::export::Print {
            tab: tab(),
            pages: None,
            dpi: 300,
            to_pdf: "/d/x.pdf".into(),
            anyway: false,
        }),
        Command::from(crate::commands::text::Text { tab: tab(), page: 1 }),
        Command::from(crate::commands::highlight::Highlight {
            tab: tab(),
            page: Some(1),
            to_page: None,
            chars: Some("0-5".into()),
            at: None,
            to: None,
            unit: None,
        }),
        Command::from(crate::commands::highlight::HighlightRemove {
            tab: tab(),
            what: crate::commands::highlight::RemoveWhat::Id(0),
        }),
        Command::from(crate::commands::highlight::Highlights { tab: tab(), page: Some(1) }),
        Command::from(crate::commands::text::Select {
            tab: tab(),
            page: Some(1),
            to_page: None,
            chars: None,
            at: None,
            to: None,
            unit: None,
            all: false,
            anyway: false,
        }),
        Command::from(crate::commands::text::Copy {
            tab: tab(),
            page: Some(1),
            to_page: None,
            chars: None,
            at: None,
            to: None,
            unit: None,
            all: false,
            anyway: false,
        }),
        Command::from(crate::commands::measure::BudgetRender {
            tab: tab(),
            pages: 1,
            first: None,
            scale: 2.0,
            fresh: false,
            tiles: false,
            samples: false,
            limit_mib: None,
            retire_mib: None,
        }),
        Command::from(crate::commands::measure::BudgetViewport {
            tab: tab(),
            viewport: "10x10".into(),
            first: None,
            scale: 2.0,
            strategy: None,
        }),
        Command::from(crate::commands::reading::Where { tab: tab() }),
        Command::from(crate::commands::reading::Facts { tab: tab() }),
        Command::from(crate::commands::reading::Contents {
            tab: tab(),
            from: None,
            limit: None,
            depth: None,
            generated: false,
        }),
        Command::from(crate::commands::text::Search {
            tab: tab(),
            query: "x".into(),
            pages: None,
            limit: None,
            scan: false,
            regex: false,
            rank: false,
            context: None,
        }),
        Command::from(crate::commands::finding::Find::from(crate::commands::finding::FindAsk::Query("x".into()))),
        Command::from(crate::commands::text::Index { tab: tab(), agent: false }),
        Command::from(crate::commands::view::Palette {
            query: Some("zoom".into()),
            run: false,
            limit: None,
        }),
        Command::from(crate::commands::text::IndexStatus { tab: tab() }),
        Command::from(crate::commands::text::IndexClear { tab: tab(), all: false }),
        Command::from(crate::commands::reading::Signatures { tab: tab() }),
        Command::from(crate::commands::editing::Edits { tab: tab() }),
        Command::from(crate::commands::editing::PageMove {
            tab: tab(),
            pages: "1".into(),
            to: 1,
        }),
        Command::from(crate::commands::editing::PageRotate {
            tab: tab(),
            pages: "1".into(),
            degrees: 90,
        }),
        Command::from(crate::commands::editing::PageDelete { tab: tab(), pages: "1".into() }),
        Command::from(crate::commands::editing::PageInsert {
            tab: tab(),
            path: "/d/b.pdf".into(),
            at: 1,
        }),
        Command::from(crate::commands::editing::Undo { tab: tab() }),
        Command::from(crate::commands::editing::Redo { tab: tab() }),
        Command::from(crate::commands::editing::Discard { tab: tab() }),
        Command::from(crate::commands::editing::ClearHistory { tab: tab() }),
        Command::from(crate::commands::edit_mode::EditMode {
            set: commands::words::OnOff::On,
        }),
        Command::from(crate::commands::contents::ContentsFold { entry: 0, tab: tab() }),
        Command::from(crate::commands::contents::ContentsShow {
            which: commands::contents::Which::Generated,
            tab: tab(),
        }),
        Command::from(crate::commands::contents::ContentsStop { tab: tab() }),
        Command::from(crate::commands::record::SessionSleep { tab: tab() }),
        Command::from(crate::commands::saving::Save {
            tab: tab(),
            out: None,
            replace: false,
        }),
        Command::from(commands::access::Sign {}),
        Command::from(commands::session::Status {}),
        Command::from(commands::access::PermissionShow {}),
        Command::from(commands::access::PermissionRequest {
            level: Level::Full,
            timeout_ms: None,
        }),
        Command::from(commands::access::LevelChanged {}),
        Command::from(commands::sync::Wait {
            condition: Condition::TabOpen,
            tab: tab(),
            page: None,
            timeout_ms: 1,
        }),
        Command::from(commands::sync::Watch {
            events: None,
            timeout_ms: None,
            count: None,
        }),
        Command::from(commands::sync::WatchStop {}),
        Command::from(commands::access::Log { last: None }),
        Command::from(crate::commands::view::View {
            paper: None,
            sidebar: None,
            agent_icon: None,
            zoom: None,
            layout: None,
            tab: None,
        }),
        Command::from(crate::commands::qa::QaRead {
            verb: "state".into(),
            out: None,
            timeout_ms: None,
        }),
        Command::from(crate::commands::qa::QaDrive {
            verb: "zoom".into(),
            value: "1".into(),
            timeout_ms: None,
            tab: None,
        }),
        Command::from(commands::session::Stop {}),
        Command::from(crate::commands::record::SessionShow {}),
        Command::from(crate::commands::record::SessionSave {}),
        Command::from(crate::commands::record::SessionRestore {}),
        Command::from(crate::commands::record::SessionCheck {}),
        Command::from(commands::updates::UpdateStatus {}),
        Command::from(commands::updates::UpdateCheck {}),
        Command::from(commands::updates::UpdateInstall { zip: None, now: false }),
        Command::from(commands::updates::UpdateDevBuilds { set: None }),
        Command::from(commands::updates::Settings { auto_update: None }),
    ]
}

const LEVELS: [Level; 4] = [Level::Off, Level::ReadOnly, Level::Control, Level::Full];

/// A PDF that stands for one read with its password: `secret`'s own length and words, each page with an `x` on it for Find to find.
fn secret_pdf(secret: u32) -> Vec<u8> {
    let count = 7 + 2 * secret as usize;
    pages(
        &(0..count)
            .map(|at| Page::new(text_lines(&[&format!("x secret {secret}, page {}", at + 1)])))
            .collect::<Vec<_>>(),
    )
}

/// Which tab is being read, and which one the command names, when it names one: the protected `#1` or the plain `#2`.
#[derive(Clone, Copy, Debug)]
struct Arrangement {
    reading_locked: bool,
    naming_locked: bool,
}

/// The three arrangements: the protected tab read and named; read and the plain one named, so a command that reads the document being read whatever it
/// names (`Reading`) is told from one that reads the tab it names (`Its`); and the plain one read with the protected one named.
const ARRANGEMENTS: [Arrangement; 3] = [
    Arrangement {
        reading_locked: true,
        naming_locked: true,
    },
    Arrangement {
        reading_locked: true,
        naming_locked: false,
    },
    Arrangement {
        reading_locked: false,
        naming_locked: true,
    },
];

/// `command` naming the plain tab (`#2`) in place of the protected one (`#1`), as its line says it.
fn naming_the_plain_tab(command: &Command) -> Command {
    let mut fields = fields(command);
    for value in fields.values_mut() {
        if value == "#1" {
            *value = Value::from("#2");
        }
    }
    crate::parse(&Value::Object(fields).to_string()).expect("its own line, naming the other tab")
}

/// What an agent at `level` is told by `command`, and what it hears, with `locked.pdf` (`#1`) opened with its password beside a plain tab (`#2`), as
/// `arranged`: a fresh session each time, so what one command did can't hide what another gave. The files' folders, which differ from run to run, are
/// said alike.
fn told(secret: u32, level: Level, command: &Command, arranged: Arrangement) -> String {
    let locked = TempPdf::new("locked.pdf", &secret_pdf(secret));
    let plain = TempPdf::new("plain.pdf", &pdf(3));
    let mut session = Session::new(crate::DrawableLoader);
    let id = session.open(&locked.0).unwrap();
    let other = session.open(&plain.0).unwrap();
    session.tabs_mut().note_password_used(id);
    session.tap(if arranged.reading_locked { id } else { other }).unwrap();
    let command = if arranged.naming_locked {
        command.clone()
    } else {
        naming_the_plain_tab(command)
    };
    let access = Mutex::new(session);
    let mut events = Vec::new();
    let result = execute(&access, Actor::Agent(level), &command, &mut events).map(|mut answer| {
        unmeasured(&mut answer);
        answer
    });
    format!("{result:?} {events:?}")
        .replace(locked.folder().to_str().unwrap(), "<locked>")
        .replace(plain.folder().to_str().unwrap(), "<plain>")
}

/// An answer with what's measured as it runs left out (times, the process's and a worker's memory, open handles), which differ from run to run
/// whatever the document. Everything else is compared, the files' sizes included: the twins' protected documents differ in length, words and size.
fn unmeasured(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, field) in fields.iter_mut() {
                if key == "ms" || key.ends_with("_ms") || key.ends_with("footprint_bytes") || matches!(key.as_str(), "worker_peak_bytes" | "open_handles") {
                    *field = Value::Null;
                } else {
                    unmeasured(field);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(unmeasured),
        _ => {}
    }
}

/// Failure (agent report §5.5, R4.2 P1 and P2): something of a document the person opened with its password reaches an agent at some level, by a command
/// added later or one whose gate was forgotten: its length, place, size or words in an answer, a refusal or an event. Every command runs at every level,
/// in each of [`ARRANGEMENTS`], on two sessions whose protected document differs, and must say the same on both. A command that reads a tab and says
/// [`commands::About::Nothing`], or reads the document being read and says `Its`, fails here, not only where someone remembers to look.
#[test]
fn no_command_tells_an_agent_anything_of_a_protected_document() {
    let mut differs = Vec::new();
    for command in every_command() {
        for level in LEVELS {
            for arranged in ARRANGEMENTS {
                let (one, two) = (told(1, level, &command, arranged), told(2, level, &command, arranged));
                if one != two {
                    differs.push(format!("{} at {level:?}, {arranged:?}:\n  {one}\n  {two}", command.tag()));
                }
            }
        }
    }
    assert!(differs.is_empty(), "told something of the protected document:\n{}", differs.join("\n"));
}

/// A real PDF of `count` pages, each with its number in words.
fn pdf(count: usize) -> Vec<u8> {
    pages(&(0..count).map(|at| Page::new(text_lines(&[&format!("Page {}", at + 1)]))).collect::<Vec<_>>())
}

/// A real session on `file`, opened as the person opens one: tab `#1`.
fn opened(file: &Path) -> Mutex<Session<crate::DrawableLoader>> {
    let access = Mutex::new(Session::new(crate::DrawableLoader));
    let open = Command::from(commands::tabs::Open {
        path: file.to_path_buf(),
        page: None,
    });
    execute(&access, Actor::Person, &open, &mut Vec::new()).expect("opened");
    access
}

/// How many of the owner's steps the tab's history holds, done and undone. (Counted, not compared by value: an undo keeps a step as its inverse until
/// it's redone, and moving one step between the two is what undo and redo are.)
fn owners_steps(access: &Mutex<Session<crate::DrawableLoader>>, id: recto_session::TabId) -> usize {
    let session = access.lock().unwrap();
    session.lists().edits_record(id).map_or(0, |record| {
        record
            .undo
            .iter()
            .chain(&record.redo)
            .filter(|entry| entry.by == recto_session::record::AuthorName::Owner)
            .count()
    })
}

/// Failure (SPEC decision 20, R4.2 L1): an agent below Full throws away some of the owner's unsaved edits, by a command added later or a rule
/// forgotten in one: a step of theirs, done or undone, gone from the history, or their file's bytes changed. Every command runs at ReadOnly and
/// Control, each on a fresh session whose owner has turned three pages and undone one, with the files it reads real ones (an `open`, a `page insert`
/// reach their rules); afterwards the owner's three steps are all there, and the file is byte for byte what it was, or, for `save`, which an agent at
/// Control may do by appending, everything that was in it is still there. A step pushed off the end of a full history is the history's own rule,
/// held by `an_agent_cant_push_the_owners_steps_out_of_the_history` in `recto-session`.
#[test]
fn no_command_below_full_loses_the_owners_edits() {
    let other = TempPdf::new("other.pdf", &pdf(2));
    let mut lost = Vec::new();
    for command in every_command() {
        // The files it reads, real ones.
        let mut asked = fields(&command);
        for key in READS {
            if asked.get(*key).is_some_and(Value::is_string) {
                asked.insert((*key).to_string(), Value::from(other.0.to_str().unwrap()));
            }
        }
        let command = crate::parse(&Value::Object(asked).to_string()).expect("its own line, with a real file");
        for level in [Level::ReadOnly, Level::Control] {
            let file = TempPdf::new("owners.pdf", &pdf(4));
            let bytes = std::fs::read(&file.0).unwrap();
            let access = opened(&file.0);
            let id = access.lock().unwrap().tabs().active().expect("the owner's tab");
            for page in ["1", "2", "3"] {
                let turn = Command::from(commands::editing::PageRotate {
                    tab: "#1".into(),
                    pages: page.into(),
                    degrees: 90,
                });
                execute(&access, Actor::Person, &turn, &mut Vec::new()).expect("the owner's turn");
            }
            execute(
                &access,
                Actor::Person,
                &Command::from(commands::editing::Undo { tab: "#1".into() }),
                &mut Vec::new(),
            )
            .expect("the owner's undo");
            assert_eq!(owners_steps(&access, id), 3, "the owner's three steps, one undone");
            let result = execute(&access, Actor::Agent(level), &command, &mut Vec::new());
            let after = owners_steps(&access, id);
            let now = std::fs::read(&file.0).unwrap();
            let kept = if command.tag() == "save" { now.starts_with(&bytes) } else { now == bytes };
            if after != 3 || !kept {
                lost.push(format!(
                    "{} at {level:?}: {result:?}\n  the owner's steps now {after}, the file kept: {kept}",
                    command.tag()
                ));
            }
        }
    }
    assert!(lost.is_empty(), "the owner's edits lost below Full:\n{}", lost.join("\n"));
}

/// The arguments of kind [`crate::table::Kind::Path`] a command writes a new file to, and those it reads one from. Every such argument is one or the
/// other ([`every_file_argument_is_read_or_written`]).
const WRITES: &[&str] = &["out", "to_pdf"];
const READS: &[&str] = &["path", "zip"];

/// Failure: a session command's file argument is neither a file it reads nor one it writes as far as these walks know, so the walks below never try it.
/// QA's verbs are the app's, and their files its own wall's (`recto-app`'s `core/qa`).
#[test]
fn every_file_argument_is_read_or_written() {
    let unknown: Vec<String> = crate::table::TABLE
        .iter()
        .filter(|spec| spec.route == crate::table::Route::Session && !spec.tag.starts_with("qa_"))
        .flat_map(|spec| spec.args.iter().map(move |arg| (spec.name, arg)))
        .filter(|(_, arg)| arg.kind == crate::table::Kind::Path && !WRITES.contains(&arg.key) && !READS.contains(&arg.key))
        .map(|(name, arg)| format!("{name}: {}", arg.key))
        .collect();
    assert!(unknown.is_empty(), "file arguments in no list: {unknown:?}");
}

/// The command's fields, as it goes on the wire.
fn fields(command: &Command) -> serde_json::Map<String, Value> {
    match serde_json::from_str(&command.wire()) {
        Ok(Value::Object(fields)) => fields,
        _ => panic!("{} isn't an object on the wire", command.tag()),
    }
}

/// `fields` with each file the command writes put in `folder`, named for its command, with the extension it had (a save's, `.pdf`).
fn writing_into(fields: &serde_json::Map<String, Value>, folder: &Path, tag: &str) -> (Command, Vec<std::path::PathBuf>) {
    let mut fields = fields.clone();
    let mut files = Vec::new();
    let keys: Vec<&str> = WRITES.iter().copied().filter(|key| fields.contains_key(*key)).collect();
    for key in keys {
        let extension = fields[key]
            .as_str()
            .and_then(|was| Path::new(was).extension())
            .map_or("pdf".into(), |ext| ext.to_string_lossy().into_owned());
        let file = folder.join(format!("{tag}-{key}.{extension}"));
        fields.insert(key.to_string(), Value::from(file.to_str().unwrap()));
        files.push(file);
    }
    (
        crate::parse(&Value::Object(fields).to_string()).expect("its own line, with another file"),
        files,
    )
}

/// Failure (AGENTS.md rule 5, R4.2 W1): a command writes over a file that's there, by a command added later that writes its file its own way: every
/// command that writes a file, given one that's there, at Full, refuses with `output_exists` or `target_exists` and leaves its bytes as they were.
#[test]
fn no_command_writes_over_a_file_thats_there() {
    let folder = recto_testpdf::TempDir::new("recto-no-overwrite-");
    let mut tried = Vec::new();
    for command in every_command().into_iter().filter(|command| command.route() == crate::Runs::Session) {
        let fields = fields(&command);
        if !WRITES.iter().any(|key| fields.contains_key(*key)) {
            continue;
        }
        let (command, files) = writing_into(&fields, &folder, command.tag());
        for file in &files {
            std::fs::write(file, b"precious").unwrap();
        }
        let doc = TempPdf::new("mine.pdf", &pdf(2));
        let result = execute(&opened(&doc.0), Actor::Agent(Level::Full), &command, &mut Vec::new());
        for file in &files {
            assert_eq!(std::fs::read(file).unwrap(), b"precious", "{} wrote over {}", command.tag(), file.display());
        }
        assert!(
            matches!(&result, Err(error) if error.code == crate::Code::OUTPUT_EXISTS || error.code == crate::Code::TARGET_EXISTS),
            "{}: {result:?}",
            command.tag()
        );
        tried.push(command.tag());
    }
    assert!(tried.len() >= 6, "the commands that write a file: {tried:?}");
}

/// A PDF whose permissions ask that it isn't copied, opened without a password. Built in memory: lopdf's eager writer is the only one that encrypts, and
/// nothing here goes near a user's file.
#[allow(clippy::disallowed_methods)]
fn restricted() -> Vec<u8> {
    use lopdf::encryption::{EncryptionState, EncryptionVersion, Permissions};
    let mut document = lopdf::Document::load_mem(&pdf(2)).unwrap();
    document.trailer.set(
        "ID",
        lopdf::Object::Array(vec![
            lopdf::Object::string_literal("0123456789abcdef"),
            lopdf::Object::string_literal("0123456789abcdef"),
        ]),
    );
    let state = EncryptionState::try_from(EncryptionVersion::V2 {
        document: &document,
        owner_password: "owner",
        user_password: "",
        key_length: 128,
        permissions: Permissions::all() - Permissions::COPYABLE,
    })
    .unwrap();
    document.encrypt(&state).unwrap();
    let mut out = Vec::new();
    document.save_to(&mut out).unwrap();
    out
}

/// Failure (owner, 2026-11-08: exports of a file that asks not to be copied get the same step as Copy; R4.2 P3): a command that copies a document's
/// text or writes a picture or a copy of it hands over a file that asks not to be copied without "anyway", or takes no `--anyway` at all, or refuses
/// it even with one. Without it, each is refused with `copy_restricted`, or, for `select`, gives the selection's places with its text left out. A save writes the document itself, which keeps its own permissions, so it isn't a copy.
#[test]
fn a_file_that_asks_not_to_be_copied_asks_anyway_of_every_copy() {
    let doc = TempPdf::new("restricted.pdf", &restricted());
    let mut tried = Vec::new();
    for command in every_command().into_iter().filter(|command| command.route() == crate::Runs::Session) {
        let mut fields = fields(&command);
        let copies = command.tag() != "save" && (WRITES.iter().any(|key| fields.contains_key(*key)) || matches!(command.tag(), "copy" | "select"));
        assert!(
            !copies || fields.contains_key("anyway"),
            "{} copies the document and takes no `--anyway`",
            command.tag()
        );
        if !fields.contains_key("anyway") {
            continue;
        }
        for anyway in [false, true] {
            let folder = recto_testpdf::TempDir::new("recto-anyway-");
            fields.insert("anyway".into(), Value::from(anyway));
            let (asked, _) = writing_into(&fields, &folder, command.tag());
            let result = execute(&opened(&doc.0), Actor::Agent(Level::Full), &asked, &mut Vec::new());
            // Refused, or, for a selection, its places given and its text left out.
            let withheld = match &result {
                Err(error) => error.code == crate::Code::COPY_RESTRICTED,
                Ok(answer) => answer["restricted"] == true && answer["untrusted"]["text"].is_null(),
            };
            assert_eq!(withheld, !anyway, "{} with anyway {anyway}: {result:?}", command.tag());
        }
        tried.push(command.tag());
    }
    assert!(tried.len() >= 7, "the commands that copy: {tried:?}");
}

/// What writes a file over one that's there, or moves one onto it, as a line of source can say it: `fs::write(` (with or without `std::`),
/// `File::create(`, `File::options`, `rename(`, `fs::copy(`, and an `OpenOptions` set to `.truncate(true)`, `.create(true)`, or `.write(true)` with no
/// `create_new` in its statement (the lines up to its `;`). Not seen: an aliased import (`use std::fs::write as put`), a call through a helper in
/// another crate, or a builder whose options take a variable.
const WRITES_OVER: &[&str] = &[
    "fs::write(",
    "File::create(",
    "File::options",
    "rename(",
    "fs::copy(",
    ".truncate(true)",
    ".create(true)",
    ".write(true)",
];

/// The writes that replace a file on purpose, each with why: by its file, and the words of the line.
const REPLACES_ON_PURPOSE: &[(&str, &str, &str)] = &[
    // The agent skill's own file in the folder `recto skill --install` was pointed at, sealed, replaced whole only when it's the one this made
    // (`skill.rs` checks the seal first); never a document.
    (
        "skill.rs",
        "std::fs::rename(&temp, path)",
        "the skill file this made, replaced whole after its seal is checked",
    ),
];

/// A source's lines outside test items: an item under `#[cfg(test)]` is left out to its end, braces counted as written (as `tests/codes.rs` reads them).
/// An item with no body (a `mod x;`, a field, a variant) ends at its `;` or `,`.
fn production_lines(source: &str) -> Vec<(usize, &str)> {
    let mut lines = Vec::new();
    let mut skipping: Option<(bool, i64)> = None;
    for (at, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        if skipping.is_none() {
            if trimmed.starts_with("#[cfg(test)]") {
                skipping = Some((false, 0));
                if trimmed.trim_end() == "#[cfg(test)]" {
                    continue;
                }
            } else {
                lines.push((at, line));
                continue;
            }
        }
        let Some((opened, depth)) = skipping.as_mut() else { continue };
        if !*opened && trimmed.starts_with("#[") {
            continue;
        }
        for c in line.chars() {
            match c {
                '{' => {
                    *opened = true;
                    *depth += 1;
                }
                '}' => *depth -= 1,
                _ => {}
            }
        }
        if (*opened && *depth <= 0) || (!*opened && (line.trim_end().ends_with(';') || line.trim_end().ends_with(','))) {
            skipping = None;
        }
    }
    lines
}

/// Failure (review of R4.2): a `#[cfg(test)]` on a field or a variant skipped every production line after it up to the next `;`, so a write there went
/// unseen. Test items are left out to their end, and the production lines after them are read.
#[test]
fn a_test_only_field_doesnt_hide_the_lines_after_it() {
    let source = "struct A {\n    #[cfg(test)]\n    seen: u8,\n    kept: u8,\n}\nfn b() {\n    let _ = std::fs::write(x, y);\n}\n#[cfg(test)]\nmod tests {\n    fn c() {}\n}\nfn d() {}\n";
    let read: Vec<&str> = production_lines(source).into_iter().map(|(_, line)| line.trim()).collect();
    assert_eq!(
        read,
        ["struct A {", "kept: u8,", "}", "fn b() {", "let _ = std::fs::write(x, y);", "}", "fn d() {}"]
    );
}

/// The file and line of each write in `text`'s production lines that could go over a file, less those replacing one on purpose.
fn writes_over(name: &str, text: &str) -> Vec<(usize, String)> {
    let lines = production_lines(text);
    let mut found = Vec::new();
    for (index, (at, line)) in lines.iter().enumerate() {
        let code = line.split("//").next().unwrap_or_default();
        let Some(form) = WRITES_OVER.iter().find(|form| code.contains(**form)) else {
            continue;
        };
        // A `.write(true)` is a write over a file only if its statement doesn't make the file new.
        if *form == ".write(true)" {
            let statement: String = lines[index.saturating_sub(4)..]
                .iter()
                .take(12)
                .map(|(_, line)| *line)
                .collect::<Vec<_>>()
                .join("\n");
            if statement.contains("create_new(true)") {
                continue;
            }
        }
        if REPLACES_ON_PURPOSE.iter().any(|(file, words, _)| name == *file && code.contains(words)) {
            continue;
        }
        found.push((at + 1, line.trim().to_string()));
    }
    found
}

/// Failure (AGENTS.md rule 5, R4.2 W1): code in `recto-agent` writes a file its own way, which can go over a file that's there, the person's own
/// included. The early word, before any walk: no production line of the crate's sources writes or moves a file but through `OwnTemp` and `publish`
/// (which make it new and never replace anything; a save goes through the lopdf writers in `recto-core`), or is in [`REPLACES_ON_PURPOSE`] with why.
/// [`no_command_writes_over_a_file_thats_there`] is the guard that counts: it holds what a command does, whatever its code says.
#[test]
fn nothing_in_the_crate_writes_a_file_but_through_its_one_way() {
    let mut folders = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
    let (mut read, mut found) = (0, Vec::new());
    while let Some(folder) = folders.pop() {
        for path in std::fs::read_dir(&folder).unwrap().map(|entry| entry.unwrap().path()) {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                if name != "tests" {
                    folders.push(path);
                }
                continue;
            }
            // Test-only files: this one, and those registered under `#[cfg(test)]` or named for tests.
            if !name.ends_with(".rs") || name.ends_with("tests.rs") || matches!(name.as_str(), "policy_guards.rs" | "registry_golden.rs" | "event_agreement.rs")
            {
                continue;
            }
            read += 1;
            let text = std::fs::read_to_string(&path).unwrap();
            found.extend(
                writes_over(&name, &text)
                    .into_iter()
                    .map(|(line, words)| format!("{}:{line}: {words}", path.display())),
            );
        }
    }
    assert!(read > 50, "the crate's sources were read: {read} files");
    assert!(
        found.is_empty(),
        "a file written its own way, which can go over one that's there; make it with `render::OwnTemp::create` and put it in place with \
         `render::publish_picture`, which never replace anything:\n{}",
        found.join("\n")
    );
    // Each exception is still there to be excepted: one whose line went is taken out of the list.
    for (file, words, _) in REPLACES_ON_PURPOSE {
        let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(file)).unwrap();
        assert!(text.contains(words), "{file} no longer has `{words}`: take it out of REPLACES_ON_PURPOSE");
    }
}

/// Failure: the scan misses a form it says it sees, or takes a write that makes its file new for one that doesn't.
#[test]
fn the_scan_sees_each_form_it_names() {
    for form in [
        "std::fs::write(out, b)",
        "fs::write(out, b)",
        "File::create(out)",
        "std::fs::File::options().append(true).open(out)",
        "std::fs::rename(a, b)",
        "std::fs::copy(a, b)",
        "OpenOptions::new().truncate(true).open(out)",
        "OpenOptions::new().create(true).open(out)",
        "OpenOptions::new().write(true).open(out)",
    ] {
        assert_eq!(writes_over("x.rs", &format!("fn f() {{\n    let _ = {form};\n}}\n")).len(), 1, "{form}");
    }
    let made_new = "fn f() {\n    let _ = OpenOptions::new()\n        .write(true)\n        .create_new(true)\n        .open(out);\n}\n";
    assert!(writes_over("x.rs", made_new).is_empty(), "create_new on the next line makes it new");
}
