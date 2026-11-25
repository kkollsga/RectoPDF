//! **Each policy a command meets, walked over every command** (R4.2, `docs/FEATURES.md`): what an agent may see of a document the person opened with its
//! password, that an agent below Full loses none of the owner's edits, that a file a command writes never goes over one that's there, and that a file
//! which asks not to be copied asks "anyway" of every command that copies or exports it. Each is decided where it's decided (the gate in
//! [`crate::execute`], the history's rules, `render`'s publishing, `extract`'s refusal); these walk [`every_command`], so a command added later is held by
//! them without a word here. The levels' own walk is `walking_every_command_at_every_level` in `lib.rs`, and the quiet run's file wall's is in
//! `quiet_files`.

use crate::actor::Stake;
use crate::{Actor, Command, Condition, Level, commands, execute};
use recto_session::Session;
use recto_testpdf::{Page, TempPdf, pages, text_lines};
use serde_json::Value;
use std::path::Path;
use std::sync::Mutex;

fn locator_page() -> recto_core::locator::Locator {
    crate::commands::locating::made_up_locator(recto_core::locator::Target::Page)
}

fn locator_spot() -> recto_core::locator::Locator {
    crate::commands::locating::made_up_locator(recto_core::locator::Target::Spot(recto_core::locator::Spot::point(10, 20).expect("a point")))
}

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
            entry: None,
            link: false,
        }),
        Command::from(crate::commands::linking::Goto {
            tab: tab(),
            page: Some(1),
            label: None,
            within: Some(0.25),
            entry: Some(0),
            link: false,
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
            at: crate::commands::locating::PageOrRef::Page(1),
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
        Command::from(crate::commands::export::Images {
            tab: tab(),
            page: 1,
            at: None,
            as_seen: None,
        }),
        Command::from(crate::commands::export::Image {
            tab: tab(),
            page: 1,
            number: Some(1),
            out: Some("/d/x.png".into()),
            anyway: false,
            at: None,
            to_clipboard: false,
            as_seen: None,
        }),
        Command::from(crate::commands::export::Snapshot {
            tab: tab(),
            page: 1,
            rect: [0.0, 0.0, 10.0, 10.0],
            format: "png".into(),
            out: Some("/d/x.png".into()),
            scale: 2.0,
            anyway: false,
            to_clipboard: false,
            as_seen: None,
        }),
        Command::from(crate::commands::export::Print {
            tab: tab(),
            pages: None,
            dpi: 300,
            to_pdf: Some("/d/x.pdf".into()),
            anyway: false,
        }),
        Command::from(crate::commands::text::Text {
            tab: tab(),
            at: crate::commands::locating::PageOrRef::Page(1),
        }),
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
        Command::from(crate::commands::locating::Ref { page: 1, chars: None, tab: tab() }),
        Command::from(crate::commands::locating::Resolve {
            reference: locator_page(),
            tab: tab(),
        }),
        Command::from(crate::commands::text::Text {
            tab: tab(),
            at: crate::commands::locating::PageOrRef::Ref(locator_page()),
        }),
        Command::from(crate::commands::export::Render {
            tab: tab(),
            at: crate::commands::locating::PageOrRef::Ref(locator_page()),
            scale: 2.0,
            out: "/d/y.png".into(),
            anyway: false,
        }),
        Command::from(crate::commands::locating::Resolve {
            reference: locator_spot(),
            tab: tab(),
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
    let (access, locked, plain) = arranged_session(secret, arranged);
    let command = if arranged.naming_locked {
        command.clone()
    } else {
        naming_the_plain_tab(command)
    };
    let mut events = Vec::new();
    let result = execute(&access, Actor::Agent(level), &command, &mut events);
    format!("{} {events:?}", said(result))
        .replace(locked.folder().to_str().unwrap(), "<locked>")
        .replace(plain.folder().to_str().unwrap(), "<plain>")
}

/// `locked.pdf` (`#1`, `secret`'s) opened with its password beside a plain tab (`#2`), the one `arranged` says being read; with the two files, which go
/// when they're dropped.
fn arranged_session(secret: u32, arranged: Arrangement) -> (Mutex<Session<crate::DrawableLoader>>, TempPdf, TempPdf) {
    let locked = TempPdf::new("locked.pdf", &secret_pdf(secret));
    let plain = TempPdf::new("plain.pdf", &pdf(3));
    let mut session = Session::new(crate::DrawableLoader);
    let id = session.open(&locked.0).unwrap();
    let other = session.open(&plain.0).unwrap();
    session.tabs_mut().note_password_used(id);
    session.tap(if arranged.reading_locked { id } else { other }).unwrap();
    (Mutex::new(session), locked, plain)
}

/// What an answer or a refusal says, with what's measured as it runs left out: from an answer, [`unmeasured`]; from a refusal's words, how long it took,
/// which a slow refusal says ([`crate::render::without_time_taken`]).
fn said(result: Result<Value, crate::AgentError>) -> String {
    let result = result
        .map(|mut answer| {
            unmeasured(&mut answer);
            answer
        })
        .map_err(|mut refused| {
            refused.message = crate::render::without_time_taken(&refused.message).to_owned();
            refused
        });
    format!("{result:?}")
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
    deadlines_met();
    let mut differs = Vec::new();
    // A synced comparison with the protected tab, which `compare`'s note and its panes' starts would tell the length of (S2): made at once, and made
    // synced afterwards.
    let synced = [
        Command::from(commands::compare::Compare::Set {
            tabs: commands::compare::Panes::new(vec![crate::TabRef::from("#1"), crate::TabRef::from("#2")]).expect("two tabs"),
            sync: Some(commands::words::OnOff::On),
        }),
        Command::from(commands::compare::Compare::Sync(commands::words::OnOff::On)),
    ];
    for command in every_command().into_iter().chain(synced) {
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

/// Failure (independent review of phase R, S1, and its follow-up on `edited`): the person's own command on a document they opened with its password is
/// heard by a watcher with something read from it: the page it turned to, its length in an edit's news, whether an edit took (deleting page 10 takes in
/// one twin and not the other), Find at a hit, a draw. Every command, and that delete, run as the person in each of [`ARRANGEMENTS`] on the two sessions
/// whose protected document differs, is heard alike once its events are past the rule every event leaving for
/// watchers meets ([`crate::withheld::for_agents`], applied where they leave, `recto_control::server::outlet`). What the person is told back isn't compared:
/// it's theirs. The walk of what the person's window, its draws and the Finder announce, over the app's socket, is `recto-app`'s
/// `an_agent_is_told_the_same_of_two_protected_documents`.
#[test]
fn no_persons_command_is_heard_with_anything_of_a_protected_document() {
    deadlines_met();
    let mut differs = Vec::new();
    // Past the end of the shorter twin (9 pages) and not of the longer (11): an edit whose taking depends on the length.
    let past_one_end = Command::from(commands::editing::PageDelete {
        tab: "#1".into(),
        pages: "10".into(),
    });
    for command in every_command().into_iter().chain([past_one_end]) {
        for arranged in ARRANGEMENTS {
            let (one, two) = (heard(1, &command, arranged), heard(2, &command, arranged));
            if one != two {
                differs.push(format!("{} as the person, {arranged:?}:\n  {one}\n  {two}", command.tag()));
            }
        }
    }
    assert!(differs.is_empty(), "heard something of the protected document:\n{}", differs.join("\n"));
}

/// What a watcher hears of `command` run as the person, in the arrangement [`told`] makes.
fn heard(secret: u32, command: &Command, arranged: Arrangement) -> String {
    let (access, locked, plain) = arranged_session(secret, arranged);
    let command = if arranged.naming_locked {
        command.clone()
    } else {
        naming_the_plain_tab(command)
    };
    let mut events = Vec::new();
    let _ = execute(&access, Actor::Person, &command, &mut events);
    crate::withheld::for_agents(&access.lock().unwrap(), None, &mut events);
    format!("{events:?}")
        .replace(locked.folder().to_str().unwrap(), "<locked>")
        .replace(plain.folder().to_str().unwrap(), "<plain>")
}

/// Where the plain tab `#2` is, beside the protected `#1` in a comparison: [`following_session`] makes each.
#[derive(Clone, Copy, Debug)]
enum Followed {
    /// Synced with `#1`, which its place follows: moved to that one's last page, as following its end would take it.
    Following,
    /// The same, then synced scroll off: left where following put it.
    Left,
    /// Left, then an agent's `goto 1` on it, which moves it and keeps its place withheld (D-A), and leaves the followed place on its trail for Back.
    LeftThenJumped,
    /// Left, then the person's own `goto 1` on it, which lets its place go (the one release there is) and leaves the followed place on its trail.
    PersonJumped,
    /// That, then the person's Back onto the followed place.
    PersonWentBack,
    /// Left, with a stop the person made before it followed, and then the person's Back to that stop, leaving the followed place on the way forward.
    PersonWentBackFromIt,
    /// Left, then put side by side with a third plain tab `#3` by the person and synced, the third scrolled three pages and the plain tab following it
    /// (the second review, S-1): where synced scroll started it, and where it is, say where following left it.
    SyncedWithAThird,
    /// Left, with the page view's exact place on the page following left it on (how far down it the protected document was) and the tab in the
    /// background behind the protected one, then an agent's `goto 9`: the page following left it on in the first twin, another in the second (the second
    /// review, S-2 and D-2: what's kept of the old place, and what the trail says; the third's D-A: the place stays withheld).
    InTheBackgroundThenJumpedToPageNine,
    /// Left, with the page view's place, then the session written to its record and restored from it, as a restart or an update's hand-off has it (the
    /// second review, S-4).
    Restarted,
}

const FOLLOWED: [Followed; 9] = [
    Followed::Following,
    Followed::Left,
    Followed::LeftThenJumped,
    Followed::PersonJumped,
    Followed::PersonWentBack,
    Followed::PersonWentBackFromIt,
    Followed::SyncedWithAThird,
    Followed::InTheBackgroundThenJumpedToPageNine,
    Followed::Restarted,
];

/// `locked.pdf` (`#1`, `secret`'s) opened with its password, and a plain 30-page `plain.pdf` (`#2`) being read beside it in a comparison, placed as
/// `followed` says: where following took it is the protected document's last page, so the twins differ in where the plain tab is, and in nothing else
/// about it. With the two files, which go when they're dropped.
fn following_session(secret: u32, followed: Followed) -> (Mutex<Session<crate::DrawableLoader>>, TempPdf, TempPdf) {
    let locked = TempPdf::new("locked.pdf", &secret_pdf(secret));
    let plain = TempPdf::new("plain.pdf", &pdf(30));
    let mut session = Session::new(crate::DrawableLoader);
    let id = session.open(&locked.0).unwrap();
    let other = session.open(&plain.0).unwrap();
    session.tabs_mut().note_password_used(id);
    if matches!(followed, Followed::PersonWentBackFromIt) {
        session.jump_as(other, 2, 0.0, recto_session::Author::Owner).unwrap();
    }
    session.compare(&[id, other], Some(other)).unwrap();
    session.set_compare_synced(true);
    let followed_to = 6 + 2 * secret;
    session.go_to(other, followed_to).unwrap();
    // How far down its page the protected one went, as the page view says it of the plain pane that follows.
    let place = recto_session::record::PlaceRecord {
        page: followed_to,
        anchor_page: followed_to,
        within: 0.3 * f64::from(secret),
        x: 0.0,
        y: 900.0 * f64::from(followed_to),
        document_height: 27_000.0,
        zoom: 1.25,
        page_width: 612.0,
        page_height: 792.0,
    };
    assert!(session.view_told_of(other, place, 1));
    if !matches!(followed, Followed::Following) {
        session.set_compare_synced(false);
    }
    match followed {
        Followed::SyncedWithAThird => {
            std::fs::write(plain.0.with_file_name("third.pdf"), pdf(30)).unwrap();
            let third = session.open(&plain.0.with_file_name("third.pdf")).unwrap();
            session.compare(&[other, third], Some(third)).unwrap();
            session.set_compare_synced(true);
            session.go_to(third, 3).unwrap();
            session.go_to(other, followed_to + 3).unwrap();
        }
        Followed::InTheBackgroundThenJumpedToPageNine => {
            session.end_compare();
            session.tap(id).unwrap();
            session.jump_as(other, 8, 0.0, recto_session::Author::Agent).unwrap();
        }
        Followed::Restarted => {
            let bytes = recto_session::record::encode_record(&session.to_record(), 3);
            let read = recto_session::record::decode_record(&bytes, recto_session::record::CURRENT_VERSION).unwrap();
            session = Session::new(crate::DrawableLoader);
            session.restore(&read.record, std::collections::HashMap::new()).unwrap();
        }
        _ => {}
    }
    if matches!(followed, Followed::LeftThenJumped | Followed::PersonWentBack) {
        session.jump_as(other, 0, 0.0, recto_session::Author::Agent).unwrap();
    }
    if matches!(followed, Followed::PersonJumped) {
        session.jump_as(other, 0, 0.0, recto_session::Author::Owner).unwrap();
    }
    if matches!(followed, Followed::PersonWentBack | Followed::PersonWentBackFromIt) {
        assert!(session.step_as(other, true, recto_session::Author::Owner).unwrap());
    }
    (Mutex::new(session), locked, plain)
}

/// Failure (security review of S2's round): an agent learns where a plain tab is that follows a protected one, or was left by following one, and so that
/// document's place and length: by any command's answer or refusal, or what it hears (past the rule events leave by), at any level: `edits` and an edit's
/// or a save's answer, `back` and `forward` after a jump, a step of Find from the place, a `goto` to the page it's on (the same-page oracle), a delete
/// of that page ("now on page N"), `contents fold`'s open or shut. Every command, naming the plain tab, at every level, in each of [`FOLLOWED`], on twins
/// that differ only in where following put it, is told and heard alike.
#[test]
fn no_command_tells_an_agent_where_a_tab_that_followed_a_protected_one_is() {
    deadlines_met();
    // Page 9 is where following leaves the plain tab in the first twin, and not in the second.
    let on_the_followed_page = [
        Command::from(commands::linking::Goto {
            tab: "#1".into(),
            page: Some(9),
            label: None,
            within: None,
            entry: None,
            link: false,
        }),
        Command::from(commands::editing::PageDelete {
            tab: "#1".into(),
            pages: "9".into(),
        }),
    ];
    let steps = [(true, false), (false, false), (true, true)]
        .map(|(forward, jump)| Command::from(commands::finding::Find::from(commands::finding::FindAsk::Step { forward, jump })));
    // Each command whose page can be left out, with it left out, so nothing that would default to where the tab is goes unseen; and Find's hit by its
    // index.
    let tab = || crate::TabRef::from("#1");
    let unpaged = [
        Command::from(commands::highlight::Highlights { tab: tab(), page: None }),
        Command::from(commands::text::Select {
            tab: tab(),
            page: None,
            to_page: None,
            chars: None,
            at: None,
            to: None,
            unit: None,
            all: false,
            anyway: false,
        }),
        Command::from(commands::text::Copy {
            tab: tab(),
            page: None,
            to_page: None,
            chars: None,
            at: None,
            to: None,
            unit: None,
            all: false,
            anyway: false,
        }),
        Command::from(commands::highlight::Highlight {
            tab: tab(),
            page: None,
            to_page: None,
            chars: Some("0-5".into()),
            at: None,
            to: None,
            unit: None,
        }),
        Command::from(commands::highlight::HighlightRemove {
            tab: tab(),
            what: commands::highlight::RemoveWhat::Pick {
                page: None,
                to_page: None,
                chars: None,
                at: None,
                to: None,
            },
        }),
        Command::from(commands::finding::Find::from(commands::finding::FindAsk::Hit(0))),
    ];
    let commands: Vec<Command> = every_command()
        .into_iter()
        .chain(on_the_followed_page)
        .chain(steps)
        .chain(unpaged)
        .map(|command| naming_the_plain_tab(&command))
        .collect();
    // Each arrangement on a thread of its own, as each run is a fresh session on files of its own: the walk is about 16 s of work.
    let differs: Vec<String> = std::thread::scope(|scope| {
        let walks: Vec<_> = FOLLOWED
            .map(|followed| {
                let commands = &commands;
                scope.spawn(move || {
                    let mut differs = Vec::new();
                    for command in commands {
                        for level in LEVELS {
                            let told = |secret: u32| {
                                let (access, locked, plain) = following_session(secret, followed);
                                let mut events = Vec::new();
                                let result = execute(&access, Actor::Agent(level), command, &mut events);
                                crate::withheld::for_agents(&access.lock().unwrap(), None, &mut events);
                                format!("{} {events:?}", said(result))
                                    .replace(locked.folder().to_str().unwrap(), "<locked>")
                                    .replace(plain.folder().to_str().unwrap(), "<plain>")
                            };
                            let (one, two) = (told(1), told(2));
                            if one != two {
                                differs.push(format!("{} at {level:?}, {followed:?}:\n  {one}\n  {two}", command.tag()));
                            }
                        }
                    }
                    differs
                })
            })
            .into_iter()
            .collect();
        walks.into_iter().flat_map(|walk| walk.join().expect("a walk")).collect()
    });
    assert!(differs.is_empty(), "told where the plain tab is:\n{}", differs.join("\n"));
}

/// Every command of the walks, naming `#1`: [`every_command`] and the ones [`no_command_tells_an_agent_where_a_tab_that_followed_a_protected_one_is`]
/// adds (a goto to the followed page, each command whose page can be left out, Find's steps and its hit by index).
fn the_walks_commands() -> Vec<Command> {
    let tab = || crate::TabRef::from("#1");
    let pick = |all: bool| commands::text::Select {
        tab: tab(),
        page: None,
        to_page: None,
        chars: None,
        at: None,
        to: None,
        unit: None,
        all,
        anyway: false,
    };
    let mut walk = every_command();
    walk.extend([
        Command::from(commands::linking::Goto {
            tab: "#1".into(),
            page: Some(9),
            label: None,
            within: None,
            entry: None,
            link: false,
        }),
        Command::from(commands::highlight::Highlights { tab: tab(), page: None }),
        Command::from(pick(false)),
        Command::from(commands::finding::Find::from(commands::finding::FindAsk::Hit(0))),
    ]);
    walk.extend(
        [(true, false), (false, false), (true, true)]
            .map(|(forward, jump)| Command::from(commands::finding::Find::from(commands::finding::FindAsk::Step { forward, jump }))),
    );
    walk
}

/// Failure (the third security review; the coordinator's D-A): some command an agent sends lets the place of a tab left by following a protected one
/// go (its `goto`, `open --page`, a link, a Find hit by its index, a contents entry), and with it what follows a release (the page view's old place told
/// again, the draws before it arrives, how many of a page's tiles are drawn again) can be set against the agent's own guess, again and again. Every
/// command of the walks, run one after another by an agent at Full on one session, naming the plain tab, on twins that differ only in where following
/// left it: each answer and what's heard is the same, and after each the plain tab's place is still withheld, as long as it's open. Destructive ones
/// (a close, a stop, a restore) go last, so the rest run on the tab.
#[test]
fn no_agent_command_lets_a_withheld_place_go() {
    deadlines_met();
    let (last, first): (Vec<Command>, Vec<Command>) = the_walks_commands()
        .into_iter()
        .map(|command| naming_the_plain_tab(&command))
        .partition(|command| matches!(command.tag(), "close" | "stop" | "session_restore"));
    let twins = [1, 2].map(|secret| following_session(secret, Followed::Left));
    let plain = recto_session::TabId(2);
    let mut differs = Vec::new();
    let mut let_go = None;
    for command in first.iter().chain(&last) {
        let [one, two] = [&twins[0], &twins[1]].map(|(access, locked, plain_file)| {
            let mut events = Vec::new();
            let result = execute(access, Actor::Agent(Level::Full), command, &mut events);
            let session = access.lock().unwrap();
            crate::withheld::for_agents(&session, None, &mut events);
            let kept = session
                .tabs()
                .tab(plain)
                .is_none_or(|_| recto_session::compare::place_withheld(&session, plain));
            let told = format!("{} {events:?}", said(result))
                .replace(locked.folder().to_str().unwrap(), "<locked>")
                .replace(plain_file.folder().to_str().unwrap(), "<plain>");
            (told, kept)
        });
        if one.0 != two.0 {
            differs.push(format!("{}:\n  {}\n  {}", command.tag(), one.0, two.0));
        }
        if !(one.1 && two.1) && let_go.is_none() {
            let_go = Some(command.tag());
        }
    }
    assert_eq!(let_go, None, "the first agent's command that let the withheld place go");
    assert!(differs.is_empty(), "told where the plain tab is:\n{}", differs.join("\n"));
}

/// Failure (the fourth security review, item 2; the coordinator's decision of 2026-11-14): the person's click on a link lets a withheld place go, so an
/// agent, which can list the document's links and the page each is on, learns from the place given after it which page following had left the tab on.
/// Each of the person's jumps as the window sends it, on a plain tab left by following: a page by its number (`goto`), by what's typed in the go-to field
/// (`goto label`) and a contents row naming no entry (`goto --within`) let it go; a link (`goto --link`), and `follow` as the person, keep it withheld.
#[test]
fn the_persons_link_keeps_a_withheld_place_withheld() {
    let plain = recto_session::TabId(2);
    let goto = |page: Option<u32>, label: Option<&str>, within: Option<f64>, link: bool| {
        Command::from(commands::linking::Goto {
            tab: crate::TabRef::Id(plain),
            page,
            label: label.map(str::to_string),
            within,
            entry: None,
            link,
        })
    };
    for (route, command, lets_go) in [
        ("a page by its number", goto(Some(4), None, None, false), true),
        ("the go-to field", goto(None, Some("4"), None, false), true),
        ("a contents row", goto(Some(4), None, Some(0.3), false), true),
        ("a link", goto(Some(4), None, Some(0.3), true), false),
        ("a link to the top of a page", goto(Some(4), None, None, true), false),
    ] {
        let (access, _locked, _plain) = following_session(1, Followed::Left);
        execute(&access, Actor::Person, &command, &mut Vec::new()).unwrap_or_else(|error| panic!("{route}: {error:?}"));
        let session = access.lock().unwrap();
        assert_eq!(!recto_session::compare::place_withheld(&session, plain), lets_go, "{route}: lets the place go");
        assert_eq!(session.tabs().tab(plain).unwrap().page, 3, "{route}: the person went where they asked");
    }
}

/// Failure (security review of S2's round): a command that starts from where a tab is says it doesn't ([`commands::Cmd::starts_from`]), so an agent's
/// runs while that place follows a protected document's and tells it; or one that doesn't is refused for it. Each of [`every_command`], and each way of
/// asking Find, against the reviewed list: Back and Forward start from the place to a stop on the trail; Find's steps, and folding a contents section
/// (against what following the place opens), from the place; every other command names its pages, or none, and a Find hit chosen by its index names
/// its hit, and moves the tab as a jump from the place, which keeps a withheld place withheld, the person's too (the fourth review's item 1).
#[test]
fn what_starts_from_the_place_is_as_listed() {
    use commands::FromPlace;
    use commands::finding::{Find, FindAsk};
    let listed = |command: &Command| match (command.tag(), command) {
        ("back", _) => FromPlace::Trail { back: true },
        ("forward", _) => FromPlace::Trail { back: false },
        ("contents_fold", _) => FromPlace::Place,
        _ => FromPlace::No,
    };
    for command in every_command() {
        assert_eq!(command.starts_from(), listed(&command), "{}", command.tag());
    }
    for (ask, from) in [
        (FindAsk::Query("x".into()), FromPlace::No),
        (FindAsk::Open, FromPlace::No),
        (FindAsk::Close, FromPlace::No),
        (FindAsk::Look, FromPlace::No),
        (FindAsk::Step { forward: true, jump: false }, FromPlace::PlaceOrHit),
        (FindAsk::Step { forward: false, jump: true }, FromPlace::PlaceOrHit),
        (FindAsk::Hit(0), FromPlace::No),
    ] {
        assert_eq!(Command::from(Find::from(ask.clone())).starts_from(), from, "find {ask:?}");
    }
}

/// Failure (the second security review, D-3): a command that starts from where a tab is runs on a thread of its own for the person (`WAITS`), so the
/// gate's look at the place ([`crate::place::refuses`]) and the command's own run are two holds of the session, and the place can be let go or withheld
/// between them. Every such command runs on the session thread, the gate and the command under one hold of it; a later command that waits and starts
/// from the place fails here.
#[test]
fn what_starts_from_the_place_runs_on_the_session_thread() {
    use commands::finding::{Find, FindAsk};
    let finds = [
        FindAsk::Step { forward: true, jump: false },
        FindAsk::Step { forward: false, jump: true },
        FindAsk::Hit(0),
    ]
    .map(|ask| Command::from(Find::from(ask)));
    let starting: Vec<Command> = every_command()
        .into_iter()
        .chain(finds)
        .filter(|command| command.starts_from() != commands::FromPlace::No)
        .collect();
    assert!(starting.len() >= 4, "Back, Forward, a step of Find and a fold were looked at");
    for command in starting {
        assert!(!command.waits(), "{} starts from the place and waits", command.tag());
    }
}

/// **The twins' answers don't turn on the machine's speed:** every renderer in this test's process gets a page deadline it always meets, so the
/// hidden-text check (a draw of the page at 2x, left unchecked past the deadline) runs to its end for both twins. Under load the check ran out of time on
/// one twin only, and the walks told them apart by it (`copy`'s `pages_unchecked`, the draw `text` notes when the check ran), failing 1 of 20 like CI
/// (2026-11-14). The check still runs: what it does is compared, not switched off.
fn deadlines_met() {
    recto_render_service::page_deadline_for_this_process(std::time::Duration::from_secs(20));
}

/// A real PDF of `count` pages, each with its number in words.
pub(crate) fn pdf(count: usize) -> Vec<u8> {
    pages(&(0..count).map(|at| Page::new(text_lines(&[&format!("Page {}", at + 1)]))).collect::<Vec<_>>())
}

/// A real session on `file`, opened as the person opens one: tab `#1`.
pub(crate) fn opened(file: &Path) -> Mutex<Session<crate::DrawableLoader>> {
    let access = Mutex::new(Session::new(crate::DrawableLoader));
    let open = Command::from(commands::tabs::Open {
        path: file.to_path_buf(),
        page: None,
    });
    execute(&access, Actor::Person, &open, &mut Vec::new()).expect("opened");
    access
}

/// A command that reads by reference, given a reference to page 1 of the file the session has open: the made-up one in [`every_command`] points at no page of
/// it, so a walk about what happens once a page is read would only see the refusal for a page that isn't there. A command with no `ref` is as it was, and one with
/// a `ref` has it rewritten to a reference that reads, which `resolve`'s entry gets as well as the commands that read a page.
fn pointing_at_page_one(access: &Mutex<Session<crate::DrawableLoader>>, command: Command) -> Command {
    let mut fields = fields(&command);
    if !fields.get("ref").is_some_and(Value::is_string) {
        return command;
    }
    let made = execute(
        access,
        Actor::Agent(Level::Full),
        &Command::from(commands::locating::Ref {
            page: 1,
            chars: None,
            tab: crate::TabRef::from("#1"),
        }),
        &mut Vec::new(),
    )
    .expect("a reference to page 1");
    fields.insert("ref".into(), made["ref"].clone());
    crate::parse(&Value::Object(fields).to_string()).expect("its own line, with a reference that reads")
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

/// The codes a refusal for the owner's edits has.
const OWNERS_CODES: [&str; 3] = ["would_clear_redo", "would_discard_user_edits", "owner_edits_at_stake"];

/// A refusal's code, words, next step and `ask`.
fn said_by(error: &crate::AgentError) -> String {
    format!("{} | {} | {} | {:?}", error.code, error.message, error.next, error.ask)
}

/// The owner's steps [`no_command_below_full_loses_the_owners_edits`] starts with.
const OWNERS: usize = 5;

/// Failure (SPEC decision 20, R4.2 L1): an agent below Full throws away some of the owner's unsaved edits, by a command added later or a rule
/// forgotten in one: a step of theirs, done or undone, gone from the history, or their file's bytes changed. Every command runs at ReadOnly and
/// Control, each on a fresh session whose owner has turned three pages and highlighted two, and undone a turn and a highlight (R4.3: a save once
/// dropped a saved highlight's steps), with the files it reads real ones (an `open`, a `page insert`
/// reach their rules); afterwards the owner's five steps are all there, and the file is byte for byte what it was, or, for `save`, which an agent at
/// Control may do by appending, everything that was in it is still there. A step pushed off the end of a full history is the history's own rule,
/// held by `an_agent_cant_push_the_owners_steps_out_of_the_history` in `recto-session`. And a refusal for the owner's edits is in the rule's own words
/// ([`Stake`]'s, said once on `Actor`), not a command's own, which could drift from them.
#[test]
fn no_command_below_full_loses_the_owners_edits() {
    let other = TempPdf::new("other.pdf", &pdf(2));
    let (mut lost, mut own_words) = (Vec::new(), Vec::new());
    let any = recto_session::TabId(1);
    let stakes: Vec<String> = [Stake::UndoneSteps(any), Stake::Discard(any), Stake::Restore, Stake::SaveDropsSteps(any)]
        .map(|stake| said_by(&stake.refusal()))
        .into();
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
            // Turns and highlights (review of R4.3): the owner's highlight is highlight 0, the one `highlight remove --id 0` names, and a turn and
            // a highlight are undone.
            let turn = |page: &str| {
                Command::from(commands::editing::PageRotate {
                    tab: "#1".into(),
                    pages: page.into(),
                    degrees: 90,
                })
            };
            let highlight = |page: u32| {
                Command::from(commands::highlight::Highlight {
                    tab: "#1".into(),
                    page: Some(page),
                    to_page: None,
                    chars: Some("0-3".into()),
                    at: None,
                    to: None,
                    unit: None,
                })
            };
            for step in [turn("1"), turn("2"), highlight(4), turn("3"), highlight(1)] {
                execute(&access, Actor::Person, &step, &mut Vec::new()).expect("the owner's step");
            }
            for _ in 0..2 {
                let undo = Command::from(commands::editing::Undo { tab: "#1".into() });
                execute(&access, Actor::Person, &undo, &mut Vec::new()).expect("the owner's undo");
            }
            assert_eq!(owners_steps(&access, id), OWNERS, "the owner's five steps, two undone");
            // A command that reads by reference points at a page of this file, so it gets as far as reading it.
            let asked = pointing_at_page_one(&access, command.clone());
            let result = execute(&access, Actor::Agent(level), &asked, &mut Vec::new());
            let after = owners_steps(&access, id);
            let now = std::fs::read(&file.0).unwrap();
            let kept = if command.tag() == "save" { now.starts_with(&bytes) } else { now == bytes };
            if after != OWNERS || !kept {
                lost.push(format!(
                    "{} at {level:?}: {result:?}\n  the owner's steps now {after}, the file kept: {kept}",
                    command.tag()
                ));
            }
            // Refused for the owner's edits, it's in the rule's own words: one of `Stake`'s, or the close rule's, which says what a close would lose.
            if let Err(error) = &result
                && OWNERS_CODES.contains(&error.code.as_str())
                && command.tag() != "close"
                && !stakes.contains(&said_by(error))
            {
                own_words.push(format!("{} at {level:?}: {error:?}", command.tag()));
            }
        }
    }
    assert!(lost.is_empty(), "the owner's edits lost below Full:\n{}", lost.join("\n"));
    assert!(
        own_words.is_empty(),
        "refused for the owner's edits in words of its own, not `actor::Stake`'s:\n{}",
        own_words.join("\n")
    );
}

/// **Failure (second review of R4.3): a save below Full that would drop the owner's steps is made**, losing them. The walk above saves only where
/// a save may be made; here the owner has highlighted page 4, saved, and deleted page 4, so their highlight's step names a page the next save
/// leaves out. An agent's `save` at Control is refused with `owner_edits_at_stake`, and the owner's steps and the file are as they were.
#[test]
fn a_save_below_full_that_would_drop_the_owners_steps_is_refused() {
    let file = TempPdf::new("owners.pdf", &pdf(4));
    let access = opened(&file.0);
    let id = access.lock().unwrap().tabs().active().expect("the owner's tab");
    let save = || {
        Command::from(commands::saving::Save {
            tab: "#1".into(),
            out: None,
            replace: false,
        })
    };
    let highlight = Command::from(commands::highlight::Highlight {
        tab: "#1".into(),
        page: Some(4),
        to_page: None,
        chars: Some("0-3".into()),
        at: None,
        to: None,
        unit: None,
    });
    let delete = Command::from(commands::editing::PageDelete {
        tab: "#1".into(),
        pages: "4".into(),
    });
    for step in [highlight, save(), delete] {
        execute(&access, Actor::Person, &step, &mut Vec::new()).expect("the owner's step");
    }
    let (steps, bytes) = (owners_steps(&access, id), std::fs::read(&file.0).unwrap());
    let refused = execute(&access, Actor::Agent(Level::Control), &save(), &mut Vec::new());
    assert!(
        refused.as_ref().is_err_and(|error| error.code == crate::Code::OWNER_EDITS_AT_STAKE),
        "{refused:?}"
    );
    assert_eq!(owners_steps(&access, id), steps, "the owner's steps are all there");
    assert_eq!(std::fs::read(&file.0).unwrap(), bytes, "and the file as it was");
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
pub(crate) fn fields(command: &Command) -> serde_json::Map<String, Value> {
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
        let session = opened(&doc.0);
        let command = pointing_at_page_one(&session, command);
        let result = execute(&session, Actor::Agent(Level::Full), &command, &mut Vec::new());
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
pub(crate) fn restricted() -> Vec<u8> {
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
            let session = opened(&doc.0);
            let asked = pointing_at_page_one(&session, asked);
            let result = execute(&session, Actor::Agent(Level::Full), &asked, &mut Vec::new());
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

/// A source's lines outside test items: the one copy, shared with the scans in other crates ([`recto_testpdf::production::production_lines`]).
pub(crate) use recto_testpdf::production::production_lines;

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

/// Failure (seen once under load, 2026-11-14): the walk above told the twins apart on how long a refusal took ("The picture couldn't be saved there." on
/// one, "... That took 183 ms." on the other), which a slow refusal adds to its words and isn't the document. A refusal whose time is on one twin only is
/// said alike, and one whose words really differ still isn't.
#[test]
fn a_refusals_time_is_left_out_and_its_words_are_not() {
    let refused = |message: &str| -> Result<Value, crate::AgentError> { Err(crate::AgentError::new(crate::Code::WRITE_FAILED, message, 1)) };
    let words = "The picture couldn't be saved there.";
    let slow = crate::render::with_time_taken_of(crate::AgentError::new(crate::Code::WRITE_FAILED, words, 1), 183);
    assert_ne!(slow.message, words, "a slow refusal says how long it took");
    assert_eq!(said(Err(slow)), said(refused(words)));
    assert_ne!(said(refused(words)), said(refused("The picture couldn't be saved here.")));
}
