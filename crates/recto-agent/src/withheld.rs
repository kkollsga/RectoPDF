//! **What of the session's events may reach an agent** (SPEC, password-protected documents: nothing read from a file opened with a password goes to an
//! agent, at any level, not its length, place, search or find hits; lists of tabs say it's protected and leave the rest out). The rule for events, stated
//! once and applied at the one place they leave for watchers (`recto_control::outlet`), whichever way they came: a command's, the person's actions on
//! the session thread, a page view's draw, the Finder's news.
//!
//! Every [`Event`] says what it's about ([`about`], an exhaustive `match` with no default), so a new event doesn't compile until someone has said which
//! it is. Whether a tab is withheld is the session's one statement of it, [`recto_session::registry::Tabs::withheld_from_agents`].

use crate::Event;
use recto_session::{DocLoader, Session, TabId};

/// What an event tells of, which decides what becomes of it for a document withheld from agents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum About {
    /// That a tab is open, showing or closed, under its file's name: what `tabs` says of a protected document too. Goes as it is.
    Listed(TabId),
    /// Something of the tab's document: where it's read, what's drawn, its layout, how it was saved, and its edits, whose taking can turn on its length
    /// (page 4 deleted only if it has one). Dropped for a withheld tab.
    Document(TabId),
    /// Find, which searches a document: the one it names, or else the one being read. Dropped while either is withheld, as its hits and how long it
    /// looks are read from it.
    Find(Option<TabId>),
    /// No document's contents: the level, how pages are shown, which tabs are side by side (as `compare` names a protected pane), and the helpers'
    /// indexes and generated contents, which never read a protected file (they stop at `needs_password` before any of it's decrypted).
    Nothing,
}

/// **What `event` is about.** No default: a new variant is classified here or doesn't compile.
pub fn about(event: &Event) -> About {
    match event {
        Event::TabOpened { tab, .. } | Event::TabActivated { tab, .. } | Event::TabClosed { tab } => About::Listed(*tab),
        Event::PageChanged { tab, .. }
        | Event::EditsChanged { tab, .. }
        | Event::Saved { tab, .. }
        | Event::PageDrawn { tab, .. }
        | Event::PageFailed { tab, .. }
        | Event::LayoutChanged { tab, .. } => About::Document(*tab),
        Event::FindRunning { tab } | Event::FindOpened { tab, .. } | Event::FindFinished { tab, .. } => About::Find(*tab),
        Event::FindClosed => About::Find(None),
        Event::IndexingStarted { .. }
        | Event::IndexingFinished { .. }
        | Event::ContentsGenerating { .. }
        | Event::ContentsGenerated { .. }
        | Event::LevelChanged { .. }
        | Event::ViewSettingsChanged { .. }
        | Event::ComparisonChanged { .. } => About::Nothing,
    }
}

/// Whether deciding what of `events` may go needs the session: only if one of them is about a document. Asked first, so a batch of nothing but the
/// level, the view and the helpers' news takes no lock.
pub fn needs_the_session(events: &[Event]) -> bool {
    events.iter().any(|event| !matches!(about(event), About::Nothing | About::Listed(_)))
}

/// **What of `events` may go to an agent,** in place: an event about a document withheld from agents is dropped; the rest go as they are, in their
/// order. `finding` is the document Find's results are about now, as its Finder holds them, so news of a search
/// still going over a protected document isn't let through because the person has since gone to another tab. Allocates nothing.
///
/// **A tab that isn't open counts as withheld here.** What's decided is decided when the news is sent, and news can come after its tab has closed (a draw
/// done before the close and told after it, a search still going over it), when the session's rule, which is about the tabs open, no longer knows it was
/// protected. So late news of any closed tab is held back (a plain one's too, which costs an agent nothing it's owed: that the tab closed still goes).
/// The session's own [`recto_session::Tabs::withheld_from_agents`] keeps its meaning, as every other caller asks it of a tab that's open (a command's
/// gate on the tab it selected, what a sleep file may hold, the window's model).
pub fn for_agents<L: DocLoader>(session: &Session<L>, finding: Option<TabId>, events: &mut Vec<Event>) {
    let withheld = |id: TabId| session.tabs().tab(id).is_none() || session.tabs().withheld_from_agents(id);
    // Asked once a batch, and only if Find's in it.
    let mut reading_withheld = None;
    events.retain(|event| match about(event) {
        About::Nothing | About::Listed(_) => true,
        About::Document(id) => !withheld(id),
        About::Find(named) => {
            let reading = *reading_withheld.get_or_insert_with(|| session.tabs().active().is_some_and(withheld) || finding.is_some_and(withheld));
            !reading && !named.is_some_and(withheld)
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use recto_session::Session;
    use serde_json::Value;

    /// One of each variant, about tab `tab`.
    fn every_event(tab: TabId) -> Vec<Event> {
        vec![
            Event::TabOpened {
                tab,
                name: Some("a.pdf".into()),
            },
            Event::TabActivated { tab, name: None },
            Event::TabClosed { tab },
            Event::PageChanged { tab, page: 3 },
            Event::EditsChanged { tab, pages: 9, edited: true },
            Event::Saved { tab, writer: "lopdf" },
            Event::PageDrawn { tab, page: 2 },
            Event::PageFailed { tab, page: 2, code: "x" },
            Event::IndexingStarted { corpus: "ab".into() },
            Event::IndexingFinished {
                corpus: "ab".into(),
                state: "done".into(),
                code: Value::Null,
                pages: Value::Null,
            },
            Event::ContentsGenerating { file: "cd".into() },
            Event::ContentsGenerated {
                file: "cd".into(),
                state: "done",
                code: None,
                elapsed_ms: Value::Null,
                helper_peak_bytes: Value::Null,
            },
            Event::LevelChanged { level: "full", was: "off" },
            Event::ViewSettingsChanged { paper: "auto", sidebar: true },
            Event::LayoutChanged { tab, layout: "single" },
            Event::ComparisonChanged {
                panes: vec![tab],
                synced: false,
            },
            Event::FindRunning { tab: Some(tab) },
            Event::FindOpened { tab: None, page: None },
            Event::FindOpened { tab: Some(tab), page: Some(2) },
            Event::FindClosed,
            Event::FindFinished { tab: Some(tab), hits: 4 },
        ]
    }

    #[derive(Clone)]
    struct Fake;

    impl DocLoader for Fake {
        fn load(&self, _path: &std::path::Path) -> Result<recto_core::DocFacts, recto_core::OpenError> {
            Ok(recto_core::DocFacts {
                page_count: 10,
                byte_len: 1,
                encrypted: false,
                version: "1.7".into(),
                title: None,
                toc: recto_core::Toc::default(),
                renderer_only: false,
            })
        }
    }

    /// A session with two tabs: `a`, opened with a password, and `b`, not, being read.
    fn two_tabs() -> (Session<Fake>, TabId, TabId) {
        let mut session = Session::new(Fake);
        let a = session.open(std::path::Path::new("/d/a.pdf")).expect("a");
        let b = session.open(std::path::Path::new("/d/b.pdf")).expect("b");
        session.tabs_mut().note_password_used(a);
        assert_eq!(session.tabs().active(), Some(b));
        (session, a, b)
    }

    /// Failure: an event about a document the person opened with its password goes to an agent with something read from it (its place, a page drawn,
    /// its length, whether an edit took, a Find over it), or an event about another document, or about none, is held back or changed, which would take
    /// from agents what they're owed. Of every variant about the protected tab, only what `tabs` says goes (open, showing, closed); about the other tab,
    /// everything goes as it was.
    #[test]
    fn only_what_tabs_says_goes_of_a_withheld_document() {
        let (session, a, b) = two_tabs();
        let mut events = every_event(a);
        for_agents(&session, None, &mut events);
        let names: Vec<String> = events.iter().map(|event| event.to_json().to_string()).collect();
        assert_eq!(
            names,
            [
                r#"{"event":"tab_opened","tab":1,"untrusted":{"name":"a.pdf"}}"#,
                r#"{"event":"tab_activated","tab":1,"untrusted":{"name":null}}"#,
                r#"{"event":"tab_closed","tab":1}"#,
                r#"{"corpus":"ab","event":"indexing_started"}"#,
                r#"{"code":null,"corpus":"ab","event":"indexing_finished","pages":null,"state":"done"}"#,
                r#"{"event":"contents_generating","file":"cd"}"#,
                r#"{"code":null,"elapsed_ms":null,"event":"contents_generated","file":"cd","helper_peak_bytes":null,"state":"done"}"#,
                r#"{"event":"level_changed","level":"full","was":"off"}"#,
                r#"{"event":"view_changed","paper":"auto","sidebar":true}"#,
                r#"{"comparison":[1],"event":"view_changed","sync":"off"}"#,
                // Find opened and closed over the document being read, b, which isn't withheld.
                r#"{"event":"find_changed","open":true}"#,
                r#"{"event":"find_changed","open":false}"#,
            ],
            "{names:#?}"
        );
        let mut others = every_event(b);
        for_agents(&session, None, &mut others);
        assert_eq!(others, every_event(b), "nothing about a document that isn't withheld changes");
    }

    /// Failure: Find's news goes to an agent while the document being read is protected, or while Find's results are about one though the person has
    /// gone to another tab since; news naming no document, Find opened or closed, included.
    #[test]
    fn find_is_held_back_while_the_document_it_searches_or_the_one_read_is_withheld() {
        let (mut session, a, b) = two_tabs();
        let find = || vec![Event::FindOpened { tab: None, page: None }, Event::FindClosed, Event::FindRunning { tab: None }];
        let mut events = find();
        for_agents(&session, Some(a), &mut events);
        assert!(events.is_empty(), "Find's results are a's: {events:?}");
        let mut events = find();
        for_agents(&session, Some(b), &mut events);
        assert_eq!(events, find(), "Find over b, being read");
        session.tap(a).expect("a shown");
        let mut events = find();
        for_agents(&session, Some(b), &mut events);
        assert!(events.is_empty(), "a is being read: {events:?}");
    }

    /// Failure (security review of S1): news about a protected tab that comes after it's closed (a draw done before the close and told after it, Find's
    /// news from a search still over it) goes to an agent: a closed tab isn't withheld by the session's rule, which is about open tabs, and the boundary
    /// decides when the news is sent. A tab that isn't open is withheld here, for its document's news and for Find's; that it closed still goes.
    #[test]
    fn news_about_a_closed_protected_tab_is_held_back() {
        let (mut session, a, _) = two_tabs();
        session.close(a).expect("a closed");
        let mut events = vec![
            Event::TabClosed { tab: a },
            Event::PageDrawn { tab: a, page: 7 },
            Event::FindRunning { tab: Some(a) },
            Event::FindFinished { tab: Some(a), hits: 3 },
        ];
        for_agents(&session, None, &mut events);
        assert_eq!(events, [Event::TabClosed { tab: a }]);
    }

    /// Failure: a batch with nothing about a document takes the session's lock to be let through, on the session thread for every page drawn.
    #[test]
    fn only_a_batch_about_a_document_asks_the_session() {
        let tab = TabId(1);
        for event in every_event(tab) {
            let asks = matches!(about(&event), About::Document(_) | About::Find(_));
            assert_eq!(needs_the_session(std::slice::from_ref(&event)), asks, "{event:?}");
        }
        assert!(!needs_the_session(&[
            Event::TabClosed { tab },
            Event::LevelChanged { level: "off", was: "off" }
        ]));
    }
}
