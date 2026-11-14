//! **The person's passwords reach a command only by the person's own door** (PLAN, "the person can't print a document they opened with its password",
//! design point 1): what the server calls never carries them, whatever actor it's given, and the person's door gives them only to the person.

use crate::host::SessionHost;
use recto_agent::keys::PersonsKeys;
use recto_agent::{Actor, Level, SessionAccess};
use recto_core::{DocFacts, OpenError, Password, Toc};
use recto_session::DocLoader;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone)]
struct Fake;

impl DocLoader for Fake {
    fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
        Ok(DocFacts {
            page_count: 1,
            byte_len: 1,
            encrypted: false,
            version: "1.7".into(),
            title: None,
            toc: Toc::default(),
            renderer_only: false,
        })
    }
}

/// Keys that give a password for any file asked about, and count the asks: the door seeded as wide as it goes, so only the door decides.
pub(super) fn any_key(asked: &Arc<AtomicUsize>) -> PersonsKeys {
    let asked = Arc::clone(asked);
    PersonsKeys::new(move |_identity| {
        asked.fetch_add(1, Ordering::SeqCst);
        Password::new("open sesame").map(Arc::new)
    })
}

/// Failure (design point 1): the access every route of the server's builds (`Host::run`, `Host::check`) holds the person's passwords, so a command the
/// server runs as the person (the actor the server is given is only a word) reads a document they opened with one; or the person's own access gives them
/// to an agent's actor. Only the person's access holds them, and only for the person.
#[test]
fn only_the_persons_access_holds_their_keys_and_only_for_them() {
    let host = SessionHost::new(Fake);
    let asked = Arc::new(AtomicUsize::new(0));
    host.set_persons_keys(any_key(&asked));
    for actor in [Actor::Person, Actor::Agent(Level::Full), Actor::Agent(Level::ReadOnly)] {
        assert!(host.access().persons_keys(actor).is_none(), "the server's access holds keys for {actor:?}");
    }
    assert!(host.persons_access().persons_keys(Actor::Person).is_some(), "the person's door holds them");
    for level in [Level::Off, Level::ReadOnly, Level::Control, Level::Full] {
        assert!(
            host.persons_access().persons_keys(Actor::Agent(level)).is_none(),
            "the person's door gives them to an agent at {level:?}"
        );
    }
    assert_eq!(asked.load(Ordering::SeqCst), 0, "nothing looked up a key");
}

/// A watch that does nothing: only whether an access holds one is asked.
struct Unwatched;

impl recto_agent::print::PrintWatch for Unwatched {
    fn begin(&self, _tab: recto_session::TabId, _pages: u32) {}
    fn page(&self, _done: u32, _of: u32) -> bool {
        true
    }
    fn ready(&self, _file: std::fs::File) {}
    fn end(&self) {}
}

/// Failure (found building the door, 2026-11-14): a print the server runs with [`Actor::Person`] (the actor it names is only a word) gets the window's
/// watch, so it starts the window's "Preparing pages for printing… [Stop]", which nothing then ends (only the person's own route says what came of a
/// print), leaving File > Print off and the app never at rest; and with no file named it would hand the window a file to print on the person's printer.
/// Only the person's access holds the watch, as only it holds their keys.
#[test]
fn only_the_persons_access_holds_the_windows_print_watch() {
    let host = SessionHost::new(Fake);
    host.set_print_watch(Arc::new(Unwatched));
    assert!(
        host.access().print_watch(Actor::Person).is_none(),
        "the server's access holds the window's watch"
    );
    assert!(host.persons_access().print_watch(Actor::Person).is_some(), "the person's door holds it");
    assert!(
        host.persons_access().print_watch(Actor::Agent(Level::Full)).is_none(),
        "and gives it to an agent"
    );
}
