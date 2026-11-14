//! **The window's clipboard reaches a command only by the person's own door** (S2's follow-up (1), 2026-11-14): the person's snapshot to the clipboard
//! is `snapshot --to-clipboard` and `image --at x,y --to-clipboard`, the same commands an agent can send, and nothing an agent sends may reach the
//! pasteboard. The clipboard is held, as the person's keys and the window's print watch are, only by the person's access, and given there only to the
//! actor at the window.

use crate::host::{Host, SessionHost};
use recto_agent::clipboard::ClipboardSink;
use recto_agent::extract::{Clip, ClipStop};
use recto_agent::{Actor, Level, SessionAccess};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A clipboard that counts what reaches it.
#[derive(Default)]
struct Counting {
    offered: AtomicUsize,
    made: AtomicUsize,
}

impl ClipboardSink for Counting {
    fn begin(&self) -> ClipStop {
        ClipStop::default()
    }
    fn begin_paste(&self) -> ClipStop {
        ClipStop::default()
    }
    fn offer(&self, _clip: Clip) {
        self.offered.fetch_add(1, Ordering::SeqCst);
    }
    fn made(&self, _bytes: Vec<u8>) {
        self.made.fetch_add(1, Ordering::SeqCst);
    }
}

/// Failure: the access every route of the server's builds holds the window's clipboard, so a snapshot the server runs (whatever actor it names, the
/// person included, as the actor is only a word) puts what an agent chose on the person's pasteboard; or the person's own access gives it to an agent.
#[test]
fn only_the_persons_access_holds_the_windows_clipboard() {
    let host = SessionHost::new(recto_agent::DrawableLoader);
    host.set_clipboard(Arc::new(Counting::default()));
    for actor in [Actor::Person, Actor::Agent(Level::Full)] {
        assert!(
            host.access().clipboard(actor).is_none(),
            "the server's access holds the clipboard for {actor:?}"
        );
    }
    assert!(host.persons_access().clipboard(Actor::Person).is_some(), "the person's door holds it");
    for level in [Level::Off, Level::ReadOnly, Level::Control, Level::Full] {
        assert!(
            host.persons_access().clipboard(Actor::Agent(level)).is_none(),
            "the person's door gives it to an agent at {level:?}"
        );
    }
}

/// **Failure: something an agent sends reaches the person's clipboard.** With a real document open and the window's clipboard set: the box, its
/// paste's SVG and the click on an image, each sent by an agent at every level, and by the server's route as the person, are refused and reach nothing;
/// the person's own route reaches it, so the count could have seen it.
#[test]
fn nothing_an_agent_sends_reaches_the_clipboard() {
    let file = recto_testpdf::TempPdf::new(
        "clipboard.pdf",
        &recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["words to copy"]))]),
    );
    let host = SessionHost::new(recto_agent::DrawableLoader);
    let clipboard = Arc::new(Counting::default());
    host.set_clipboard(Arc::clone(&clipboard) as Arc<dyn ClipboardSink>);
    let run = |line: &str| recto_agent::parse(line).expect("a command");
    let open = format!(r#"{{"cmd":"open","path":{}}}"#, serde_json::Value::from(file.0.display().to_string()));
    host.run(Actor::Agent(Level::Full), &run(&open), &mut Vec::new()).expect("opened");
    // Each names its page as the window would, so a refusal can only be for whose it is.
    let lines = [
        r#"{"cmd":"snapshot","page":1,"rect":[10,10,300,100],"as":"png","to_clipboard":true,"anyway":true,"as_seen":{"quarter_turns":0,"file_page":0,"own":true}}"#,
        r#"{"cmd":"snapshot","page":1,"rect":[10,10,300,100],"as":"svg","to_clipboard":true,"as_seen":{"quarter_turns":0,"file_page":0,"own":true}}"#,
        r#"{"cmd":"image","page":1,"at":[20,20],"to_clipboard":true,"anyway":true,"as_seen":{"quarter_turns":0,"file_page":0,"own":true}}"#,
    ];
    assert!(
        recto_agent::parse(r#"{"cmd":"image","page":1,"at":[20,20],"anyway":true}"#).is_err(),
        "a click with no clipboard isn't a command"
    );
    for line in lines {
        for actor in [
            Actor::Agent(Level::ReadOnly),
            Actor::Agent(Level::Control),
            Actor::Agent(Level::Full),
            Actor::Person,
        ] {
            let done = Host::run(&host, actor, &run(line), &mut Vec::new());
            assert!(done.is_err(), "{line} as {actor:?} by the server's route: {done:?}");
        }
    }
    assert_eq!(
        (clipboard.offered.load(Ordering::SeqCst), clipboard.made.load(Ordering::SeqCst)),
        (0, 0),
        "an agent's command reached the clipboard"
    );
    host.run_for_person(&run(lines[0]), &mut Vec::new()).expect("the person's own box");
    assert_eq!(clipboard.offered.load(Ordering::SeqCst), 1, "the person's own box reached it");
}
