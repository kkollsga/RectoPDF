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

/// What an agent at `level` is told by `command`, and what it hears, with `locked.pdf` (`#1`) opened with its password and being read, beside a plain
/// tab: a fresh session each time, so what one command did can't hide what another gave. The files' folders, which differ from run to run, are said
/// alike.
fn told(secret: u32, level: Level, command: &Command) -> String {
    let locked = TempPdf::new("locked.pdf", &secret_pdf(secret));
    let plain = TempPdf::new("plain.pdf", &pdf(3));
    let mut session = Session::new(crate::DrawableLoader);
    let id = session.open(&locked.0).unwrap();
    session.open(&plain.0).unwrap();
    session.tabs_mut().note_password_used(id);
    session.tap(id).unwrap();
    let access = Mutex::new(session);
    let mut events = Vec::new();
    let result = execute(&access, Actor::Agent(level), command, &mut events).map(|mut answer| {
        unmeasured(&mut answer);
        answer
    });
    format!("{result:?} {events:?}")
        .replace(locked.folder().to_str().unwrap(), "<locked>")
        .replace(plain.folder().to_str().unwrap(), "<plain>")
}

/// An answer with what's measured as it runs (times, the process's memory) left out: those differ from run to run whatever the document, and the
/// twins' documents are the same size.
fn unmeasured(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, field) in fields.iter_mut() {
                if key.ends_with("_ms") || key.ends_with("_bytes") {
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
/// added later or one whose gate was forgotten: its length, place or words in an answer, a refusal or an event. Every command runs at every level
/// on two sessions whose protected document differs, and must say the same on both. A command that reads the document being read and says
/// [`commands::About::Nothing`] fails here, not only where someone remembers to look.
#[test]
fn no_command_tells_an_agent_anything_of_a_protected_document() {
    let mut differs = Vec::new();
    for command in every_command() {
        for level in LEVELS {
            let (one, two) = (told(1, level, &command), told(2, level, &command));
            if one != two {
                differs.push(format!("{} at {level:?}:\n  {one}\n  {two}", command.tag()));
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

/// How many of the owner's steps `#1`'s history holds, done and undone, and whether one of them fell off it. (Counted, not compared by value: an undo
/// keeps a step as its inverse until it's redone, and moving one step between the two is what undo and redo are.)
fn owners_steps(access: &Mutex<Session<crate::DrawableLoader>>) -> (usize, bool) {
    let session = access.lock().unwrap();
    let Some(record) = session.tabs().active().and_then(|id| session.lists().edits_record(id)) else {
        return (0, false);
    };
    let owners = record
        .undo
        .iter()
        .chain(&record.redo)
        .filter(|entry| entry.by == recto_session::record::AuthorName::Owner)
        .count();
    (owners, record.owner_step_dropped)
}

/// Failure (SPEC decision 20, R4.2 L1): an agent below Full throws away some of the owner's unsaved edits, by a command added later or a rule
/// forgotten in one: a step of theirs, done or undone, gone from the history, or their file's bytes changed. Every command runs at ReadOnly and
/// Control, each on a fresh session whose owner has turned three pages and undone one, and afterwards the owner's steps are all there, none fell off,
/// and the file is byte for byte what it was, or, for `save`, which an agent at Control may do by appending, everything that was in it is still there.
#[test]
fn no_command_below_full_loses_the_owners_edits() {
    let mut lost = Vec::new();
    for command in every_command() {
        for level in [Level::ReadOnly, Level::Control] {
            let file = TempPdf::new("owners.pdf", &pdf(4));
            let bytes = std::fs::read(&file.0).unwrap();
            let access = opened(&file.0);
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
            let before = owners_steps(&access);
            assert_eq!(before, (3, false), "the owner's three steps, one undone");
            let result = execute(&access, Actor::Agent(level), &command, &mut Vec::new());
            let after = owners_steps(&access);
            let now = std::fs::read(&file.0).unwrap();
            let kept = if command.tag() == "save" { now.starts_with(&bytes) } else { now == bytes };
            if after != before || !kept {
                lost.push(format!("{} at {level:?}: {result:?}\n  before {before:?}\n  after {after:?}", command.tag()));
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
