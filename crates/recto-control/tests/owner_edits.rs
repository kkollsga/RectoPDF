//! The rule on throwing edits away, over the real socket, with the owner's edits made the way the app will make them: on the session it holds, where no command reaches.

use recto_agent::{Level, exit};
use recto_control::StatePaths;
use recto_control::client::Client;
use recto_control::host::SessionHost;
use recto_control::levels;
use recto_control::server::{Limits, Server};
use recto_core::{DocFacts, OpenError, Toc};
use recto_session::{DocLoader, TabId};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
struct Fake;

impl DocLoader for Fake {
    fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
        Ok(DocFacts { page_count: 10, byte_len: 1, encrypted: false, version: "1.7".into(), title: None, toc: Toc::default(), renderer_only: false })
    }
}

struct Running {
    paths: StatePaths,
    host: Arc<SessionHost<Fake>>,
    thread: Option<std::thread::JoinHandle<std::io::Result<()>>>,
    stopper: recto_control::server::Stopper,
}

impl Running {
    fn at(level: Level) -> Self {
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!("roe{}", std::process::id()));
        let paths = StatePaths::overridden(&root).unwrap();
        paths.prepare().unwrap();
        levels::store(&paths, level).unwrap();
        let server = Server::bind(&paths).unwrap();
        let stopper = server.stopper().unwrap();
        let host = Arc::new(SessionHost::new(Fake));
        let hosted = Arc::clone(&host);
        let thread = std::thread::spawn(move || server.run(hosted, Limits::default()));
        Self { paths, host, thread: Some(thread), stopper }
    }

    fn call(&self, line: Value) -> Value {
        Client::connect(&self.paths).unwrap().call(&line.to_string(), Duration::from_secs(20)).unwrap()
    }

    fn refusal(&self, line: Value) -> (String, i64, String) {
        let reply = self.call(line);
        assert_eq!(reply["ok"], false, "{reply}");
        (reply["error"]["code"].as_str().unwrap().to_string(), reply["error"]["exit"].as_i64().unwrap(), reply["error"]["message"].as_str().unwrap().to_string())
    }

    /// Each page's added turns, as they stand now.
    fn turns(&self) -> Vec<u8> {
        self.host.with_session(|s| s.page_edits(TabId(1)).map_or_else(Vec::new, |edits| edits.slots().iter().map(|slot| slot.quarter_turns).collect()))
    }

    fn tab_is_open(&self) -> bool {
        self.host.with_session(|s| s.tab(TabId(1)).is_some())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stopper.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_dir_all(&self.paths.root);
    }
}

/// Failure: the attack the old rule existed for, now that an agent may clear its own edits. The owner edits; an agent at Control undoes it all, edits, undoes, closes. At every step the owner's edits must still be there, and `redo` must bring back every one, exactly.
#[test]
fn over_the_socket_an_agent_cant_erase_the_owners_edits_at_any_step() {
    let running = Running::at(Level::Control);
    assert_eq!(running.call(json!({ "cmd": "open", "path": "/d/a.pdf" }))["ok"], true);
    running.host.with_session(|s| {
        s.edit_pages(TabId(1), |edits| edits.rotate(0, 1, 1)).unwrap();
        s.edit_pages(TabId(1), |edits| edits.rotate(1, 1, 2)).unwrap();
    });
    let owners = running.turns();
    let holds_them = || running.host.with_session(|s| s.page_edits(TabId(1)).is_some_and(recto_session::edit::PageEdits::holds_owner_edits));
    let edit = || json!({ "cmd": "page_rotate", "pages": "9", "degrees": 90 });

    let (code, status, _) = running.refusal(json!({ "cmd": "discard" }));
    assert_eq!((code.as_str(), status), ("would_discard_user_edits", i64::from(exit::PERMISSION)));
    assert_eq!(running.refusal(json!({ "cmd": "close" })).0, "unsaved_changes");
    assert_eq!(running.turns(), owners, "before any undo");

    for undone in 1..=2 {
        assert_eq!(running.call(json!({ "cmd": "undo" }))["ok"], true);
        let turns_now = running.turns();
        for attempt in [edit(), json!({ "cmd": "page_delete", "pages": "9" }), json!({ "cmd": "page_move", "pages": "9", "to": 10 }), json!({ "cmd": "page_insert", "path": "/d/b.pdf", "at": 1 }), json!({ "cmd": "close" })] {
            let (code, status, message) = running.refusal(attempt.clone());
            assert_eq!((code.as_str(), status), ("would_clear_redo", i64::from(exit::PERMISSION)), "{attempt} after {undone} undone");
            assert!(message.contains("user's"), "{message}");
        }
        let (code, status, message) = running.refusal(json!({ "cmd": "discard" }));
        assert_eq!((code.as_str(), status), ("would_discard_user_edits", i64::from(exit::PERMISSION)), "after {undone} undone");
        assert!(message.contains("user's"), "{message}");
        assert_eq!(running.turns(), turns_now, "nothing moved after {undone} undone");
        assert!(holds_them() && running.tab_is_open(), "after {undone} undone");
    }
    // Failure: with them all undone, the page matches the file, and the session's own close would have lost them.
    let undone = running.call(json!({ "cmd": "where" }));
    assert_eq!((&undone["data"]["edited"], &undone["data"]["user_edits_undone"]), (&json!(false), &json!(true)), "{undone}");
    assert!(running.host.with_session(|s| s.close(TabId(1)).is_err()), "the app's route refuses too");
    // Stopping the session would end them too.
    assert_eq!(running.refusal(json!({ "cmd": "stop" })).0, "unsaved_changes");

    assert_eq!(running.call(json!({ "cmd": "redo" }))["ok"], true);
    assert_eq!(running.call(json!({ "cmd": "redo" }))["ok"], true);
    assert_eq!(running.turns(), owners, "redo gets back every one");

    // The agent's own work on top is its own: edit, undo, edit, undo. Then it can't close, because the owner's are unsaved, and those stay.
    assert_eq!(running.call(edit())["ok"], true);
    assert_eq!(running.call(json!({ "cmd": "undo" }))["ok"], true);
    assert_eq!(running.call(edit())["ok"], true, "only its own undone step goes");
    assert_eq!(running.call(json!({ "cmd": "undo" }))["ok"], true);
    let (code, _, message) = running.refusal(json!({ "cmd": "close" }));
    assert_eq!(code, "unsaved_changes");
    assert!(message.contains("user's"), "{message}");
    assert_eq!(running.turns(), owners, "nothing of the owner's has gone");
    assert!(running.tab_is_open());
}
