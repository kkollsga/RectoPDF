//! **Clear leftovers on the app's own core** (`docs/design/removal.md` §10, D5): `removal check` and `removal clear` as an agent sends them and as the person's
//! window will, through the one host, on a disk in a temp folder whose Trash is a folder (the app never touches the person's Trash outside their own state
//! folder). What the surveys, plan, leases and acts do on their own is `recto-control`'s tests; here it is the app's part: who may, one clear at a time, and the
//! hold a QA run puts a clear at.

use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::{Actor, Level};
use serde_json::Value;
use std::path::PathBuf;

const DEAD: u32 = 4_000_001;

struct Bed {
    app: HeadlessApp,
    document: PathBuf,
}

impl Bed {
    fn root(&self) -> PathBuf {
        self.app.disk().paths().root.clone()
    }

    fn trash(&self) -> PathBuf {
        self.root().join("trash-stand-in")
    }

    /// What is under the state folder and the documents' folder, with inode numbers, for "nothing was done".
    fn listing(&self) -> Vec<String> {
        use std::os::unix::fs::MetadataExt;
        fn walk(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for entry in entries {
                let path = entry.unwrap().path();
                let meta = std::fs::symlink_metadata(&path).unwrap();
                out.push(format!("{} {}", path.strip_prefix(base).unwrap().display(), meta.ino()));
                if meta.is_dir() {
                    walk(&path, base, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root(), &self.root(), &mut out);
        walk(
            &self.document.parent().unwrap().to_path_buf(),
            &self.document.parent().unwrap().to_path_buf(),
            &mut out,
        );
        out.retain(|line| {
            !line.starts_with("run/")
                && !line.contains("removal")
                && !line.contains(".lock")
                && !line.contains("record")
                && !line.contains("journals")
                && !line.contains("ledger")
                && !line.starts_with("trash-stand-in")
        });
        out.sort();
        out
    }

    fn agent(&self, level: Level, line: &str) -> Result<Value, recto_agent::AgentError> {
        self.app.agent(level, line)
    }

    fn person(&self, line: &str) -> Result<Value, recto_agent::AgentError> {
        let command = recto_agent::parse(line).expect("a command");
        self.app
            .core()
            .host
            .run_as(Actor::Person, &command, &mut Vec::new(), crate::queue::Origin::Input)
    }
}

/// An app with one document open, a crashed save of another process beside it, and the copies of the session an update and a damaged record leave.
fn bed() -> Bed {
    let disk = Disk::new();
    let document = disk.pdf("report.pdf", 2);
    let temp = format!("{}{DEAD}-1.tmp", recto_core::save::writer_temp_prefix(&document));
    std::fs::write(document.parent().unwrap().join(temp), b"a crashed save").unwrap();
    let app = disk.builder().start();
    let root = app.disk().paths().root.clone();
    for copy in ["session.damaged", "update/before"] {
        std::fs::create_dir_all(root.join(copy)).unwrap();
        std::fs::write(root.join(copy).join("record.a.json"), b"{}").unwrap();
    }
    std::fs::write(root.join("update/before/.whole"), b"").unwrap();
    app.open(&document);
    app.settle();
    // The pass over older sleep files has to have ended before a clear may go.
    app.wait_for("the sleep pass", || {
        app.core()
            .on_disk
            .sleeper()
            .older_pass_done()
            .then_some(())
            .ok_or_else(|| "the pass is still going".to_owned())
    });
    Bed { app, document }
}

const CHECK: &str = r#"{"cmd":"removal_check"}"#;

fn clear_line(plan: &str) -> String {
    format!(r#"{{"cmd":"removal_clear","confirm":"{plan}"}}"#)
}

fn plan(checked: &Value) -> String {
    checked["plan"].as_str().expect("a plan to confirm").to_owned()
}

/// Failure: an agent below Full is given a plan (so it fills the store the person's plans wait in), is let clear by any route (the level of the command,
/// the host's own check), or sees names in the words, which are the person's. A clear refused for its level does nothing.
#[test]
fn only_full_may_clear_and_a_lower_agent_is_given_no_plan_and_no_names() {
    let bed = bed();
    let before = bed.listing();
    for level in [Level::ReadOnly, Level::Control] {
        let checked = bed.agent(level, CHECK).expect("a look");
        assert!(checked.get("plan").is_none() && checked["offered"] == false, "{level:?}: {checked}");
        assert_eq!(
            (checked["found"]["saves_here"].as_u64(), checked["found"]["copies"].as_u64()),
            (Some(1), Some(2)),
            "{checked}"
        );
        assert!(
            !checked["words"].as_str().unwrap().contains("report.pdf"),
            "an agent's words have no names: {checked}"
        );
        assert_eq!(
            checked["untrusted"]["document"], "report.pdf",
            "names are the agent's only as the document's words"
        );
        for how in ["through recto", "straight to the host"] {
            let line = clear_line("0123456789abcdef");
            let refused = if how == "through recto" {
                bed.agent(level, &line)
            } else {
                let command = recto_agent::parse(&line).expect("a command");
                bed.app
                    .core()
                    .host
                    .run_as(Actor::Agent(level), &command, &mut Vec::new(), crate::queue::Origin::Input)
            };
            assert_eq!(refused.expect_err("refused").code.as_str(), "permission_denied", "{level:?} {how}");
        }
    }
    assert_eq!(bed.app.core().tools.removal().pending(), 0, "nothing below Full is kept");
    assert_eq!(bed.listing(), before, "nothing was done");
    let full = bed.agent(Level::Full, CHECK).expect("a look");
    assert!(full["plan"].is_string() && full["button"] == "Move to Trash and Delete", "{full}");
}

/// Failure: the clear through the host isn't the same clear as the library's: it doesn't move the save to the Trash, delete the copies, or it touches the
/// document, and a second check says there is still something. An agent at Full does it end to end.
#[test]
fn an_agent_at_full_clears_end_to_end() {
    let bed = bed();
    let document = std::fs::read(&bed.document).unwrap();
    let plan = plan(&bed.agent(Level::Full, CHECK).expect("a look"));
    let cleared = bed.agent(Level::Full, &clear_line(&plan)).expect("cleared");
    assert_eq!(
        (cleared["cleared"]["trashed"].as_u64(), cleared["cleared"]["copies"].as_u64()),
        (Some(1), Some(2)),
        "{cleared}"
    );
    assert_eq!(std::fs::read_dir(bed.trash()).unwrap().count(), 1, "the save is in the Trash");
    assert!(!bed.root().join("update/before").exists() && !bed.root().join("session.damaged").exists());
    assert_eq!(std::fs::read(&bed.document).unwrap(), document);
    let again = bed.agent(Level::Full, CHECK).expect("a look");
    assert!(
        again.get("plan").is_none() && again["words"].as_str().unwrap().starts_with("Nothing found"),
        "{again}"
    );
    // The same plan again is nobody's.
    assert_eq!(bed.agent(Level::Full, &clear_line(&plan)).unwrap_err().code.as_str(), "removal_unknown_plan");
}

/// Failure: a plan the person was given is good for an agent that sends it (the agent clears as the person), or an agent's for the person.
#[test]
fn a_plan_is_its_asker_s_alone() {
    let bed = bed();
    let before = bed.listing();
    let persons = plan(&bed.person(CHECK).expect("a look"));
    assert_eq!(bed.agent(Level::Full, &clear_line(&persons)).unwrap_err().code.as_str(), "removal_unknown_plan");
    assert_eq!(bed.listing(), before);
    // The person's own words carry the names.
    let checked = bed.person(CHECK).expect("a look");
    assert!(checked["words"].as_str().unwrap().contains("“report.pdf”"), "{checked}");
    let again = plan(&checked);
    assert!(bed.person(&clear_line(&again)).is_ok());
}

/// Failure: a check or a second clear during a clear is answered as if nothing were running (the plans race), or a clear held mid-way leaves the app
/// unable to say it is busy.
#[test]
fn a_check_or_a_clear_during_a_clear_is_busy_and_nothing_else_is_disturbed() {
    let bed = bed();
    let plan = plan(&bed.agent(Level::Full, CHECK).expect("a look"));
    let removal = bed.app.core().tools.removal();
    // Held before the act of the second step: the first step (the leases) is done.
    removal.hold_at(1);
    std::thread::scope(|scope| {
        let clear = scope.spawn(|| bed.agent(Level::Full, &clear_line(&plan)));
        bed.app
            .wait_for("the clear to be held", || removal.held().then_some(()).ok_or_else(|| "not held yet".to_owned()));
        assert!(removal.clearing());
        assert_eq!(bed.agent(Level::Full, CHECK).unwrap_err().code.as_str(), "removal_busy");
        assert_eq!(bed.agent(Level::Full, &clear_line(&plan)).unwrap_err().code.as_str(), "removal_busy");
        removal.release();
        let cleared = clear.join().unwrap().expect("the held clear goes on and finishes");
        assert_eq!(cleared["outcome"], "complete");
    });
    assert!(!removal.clearing());
    assert!(bed.agent(Level::Full, CHECK).is_ok(), "once it is over a check is answered");
}

/// Failure: an agent is told what is beside a document the person opened with its password (the other documents' unfinished saves in its folder, the sizes of
/// what was kept), as `removal check` ran past the protected gate that `run_app` doesn't have. The person's own check of it is answered.
#[test]
fn a_document_opened_with_its_password_is_the_persons_to_look_beside_and_no_agents() {
    use lopdf::encryption::Permissions;
    let app = HeadlessApp::start();
    let file = app.disk().file(
        "locked.pdf",
        &super::files::locked_allowing("right", 3, Permissions::PRINTABLE | Permissions::COPYABLE),
    );
    app.act(crate::core::controls::UiAction::Open {
        path: file.display().to_string(),
    });
    app.settle();
    let id = app.reader().password_prompt.expect("it asks for its password").id;
    app.core()
        .tools
        .prompts()
        .password_answered(id, Some("right".into()), &app.core().core, &app.core().window);
    app.settle();
    for level in [Level::ReadOnly, Level::Control, Level::Full] {
        assert_eq!(app.agent(level, CHECK).unwrap_err().code.as_str(), "protected", "{level:?}");
    }
    let command = recto_agent::parse(CHECK).expect("a command");
    let checked = app.core().host.run_as(Actor::Person, &command, &mut Vec::new(), crate::queue::Origin::Input);
    assert!(checked.is_ok(), "{checked:?}");
}
