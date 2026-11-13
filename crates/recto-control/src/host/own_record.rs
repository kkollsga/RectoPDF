//! **A session with no window keeps a record of its own** (`recto serve`): nothing writes until it's told to, and each save is the whole of it. What each of the record's commands answers is `recto-agent`'s, in `commands/record.rs`; this is only what this host does for them. The app keeps its record as it goes, and gives its own.
//!
//! Locks: the session's alone while a save gathers what it writes, a tab goes to sleep or a restore is taken in; the store's alone while it writes, reads back or loads. Never one inside the other: `session show`'s look at the store, which was inside the session's lock, is now before it.

use super::{kept_json, store_error};
use crate::record_store::{Loaded, RecordStore, notice_for};
use recto_agent::Code;
use recto_agent::commands::record::{HandOff, Records, Restored, Saved, Slept};
use recto_agent::{AgentError, exit};
use recto_session::held::Held;
use recto_session::{DocLoader, Session, TabId};
use serde_json::Value;

/// The session and the store it writes to.
pub(super) struct OwnRecord<'a, L> {
    pub session: &'a Held<L>,
    pub store: &'a RecordStore,
}

impl<L: DocLoader + Clone> OwnRecord<'_, L> {
    pub(super) fn with<R>(&self, step: impl FnOnce(&mut Session<L>) -> R) -> R {
        step(&mut self.session.lock())
    }
}

impl<L: DocLoader + Clone> Records for OwnRecord<'_, L> {
    fn kept(&self) -> Value {
        kept_json(self.store, None)
    }

    /// No pictures for a tab here, and no journals kept as it goes, so all it can let go of is what it's read.
    fn put_to_sleep(&self, tab: TabId, _actor: recto_agent::Actor) -> Result<Slept, AgentError> {
        Ok(self.with(|session| Slept {
            dormant: session.sleeper().go_dormant(tab),
            list_asleep: session.lists().is_asleep(tab),
            note: None,
        }))
    }

    /// No sleep files here, so whoever asks, it's the same.
    fn save(&self, _actor: recto_agent::Actor) -> Result<Saved, AgentError> {
        let started = std::time::Instant::now();
        let (record, checkpoints, lists, edited, retired) = self.with(|session| {
            // What's been discarded goes, or it would come back at the next restore.
            let retired = session.journal_desk().work().retired;
            let checkpoints = session.journal_desk().checkpoints();
            let lists = session.journal_look().lists_by_journal();
            let edited = session.lists().edited_tabs();
            (session.to_record(), checkpoints, lists, edited, retired)
        });
        for name in &retired {
            self.store.remove_journal(name);
        }
        for (name, header, list) in &checkpoints {
            self.store.write_checkpoint(name, header, list).map_err(store_error)?;
        }
        self.store.next_write_whole();
        self.store
            .write_record(&record, &recto_session::record::PlaceDelta::default(), true)
            .map_err(store_error)?;
        self.store.verify(&record, &lists).map_err(store_error)?;
        let (record_bytes, journal_bytes) = self.store.sizes();
        Ok(Saved {
            tabs: record.tabs.len(),
            edited_tabs: edited,
            record_bytes,
            journal_bytes,
            wrote_bytes: self.store.wrote_bytes(),
            total_us: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
            timings: None,
        })
    }

    fn restore(&self) -> Result<Restored, AgentError> {
        let loaded = self.store.load();
        let notice = notice_for(&loaded).and_then(recto_session::record::notices::words);
        let Loaded::Restored(restored) = loaded else {
            return Ok(Restored { restored: false, notice });
        };
        self.with(|session| {
            session
                .restore(&restored.record, restored.journals)
                .map_err(|error| AgentError::new(Code::lower(error.code()), "The record can't be used.", exit::FAILED))?;
            // The active tab's facts are what a session with no window would otherwise have to find out at its next look.
            session.wake_active();
            Ok(Restored { restored: true, notice })
        })
    }

    /// Nothing here is the person's, or a sheet, so any moment is a safe one.
    fn hand_off(&self) -> HandOff {
        HandOff {
            safe: true,
            busy: Vec::new(),
            seconds_since_input: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::StatePaths;
    use crate::host::{Host, SessionHost};
    use recto_agent::{AgentError, Level, parse};
    use recto_core::{DocFacts, OpenError, Toc};
    use recto_session::DocLoader;
    use serde_json::Value;
    use std::path::Path;

    #[derive(Clone)]
    struct Fake;

    impl DocLoader for Fake {
        fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
            Ok(DocFacts {
                page_count: 10,
                byte_len: 1,
                encrypted: false,
                version: "1.7".into(),
                title: None,
                toc: Toc::default(),
                renderer_only: false,
            })
        }
    }

    fn run(host: &SessionHost<Fake>, level: Level, line: &str) -> Result<Value, AgentError> {
        host.run(recto_agent::Actor::Agent(level), &parse(line).expect("a command"), &mut Vec::new())
    }

    fn with_record() -> (SessionHost<Fake>, recto_testpdf::TempDir) {
        let root = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().expect("temp"), "own");
        let paths = StatePaths::overridden(&root).expect("paths");
        paths.prepare().expect("prepared");
        (SessionHost::new(Fake).with_own_record(&paths), root)
    }

    /// Failure: a session with no window answers the record's commands otherwise than it did when it answered them itself, now that `recto-agent` makes the answers: its hand-off check stops saying any moment is safe, a save says timings it never measured, `session show` stops seeing what was saved, or a restore of what was saved loses a tab or which one was active. Or the level, which only the server checked before `execute` ran these, isn't checked by the host.
    #[test]
    fn a_session_with_no_window_answers_from_its_own_record() {
        let (host, _root) = with_record();
        for name in ["a", "b"] {
            run(&host, Level::Full, &format!(r#"{{"cmd":"open","path":"/d/{name}.pdf"}}"#)).expect("opened");
        }
        let check = run(&host, Level::ReadOnly, r#"{"cmd":"session_check"}"#).expect("check");
        assert_eq!(check, serde_json::json!({ "safe": true, "busy": [], "seconds_since_input": null }));
        let show = run(&host, Level::ReadOnly, r#"{"cmd":"session_show"}"#).expect("show");
        assert_eq!((show["record"]["kept"].clone(), show["count"].clone()), (false.into(), 2.into()), "{show}");
        let slept = run(&host, Level::Control, r##"{"cmd":"session_sleep","tab":"#1"}"##).expect("slept");
        assert_eq!(slept["tab"], 1, "{slept}");
        assert!(slept["dormant"].is_boolean() && slept["list_asleep"].is_boolean(), "{slept}");
        let saved = run(&host, Level::Control, r#"{"cmd":"session_save"}"#).expect("saved");
        assert_eq!((saved["saved"].clone(), saved["tabs"].clone()), (true.into(), 2.into()), "{saved}");
        assert!(saved["total_ms"].is_u64(), "{saved}");
        assert!(
            saved.get("total_us").is_none() && saved.get("write_us").is_none(),
            "no timings it didn't measure: {saved}"
        );
        let show = run(&host, Level::ReadOnly, r#"{"cmd":"session_show"}"#).expect("show");
        assert_eq!(show["record"]["kept"], true, "{show}");
        let restored = run(&host, Level::Control, r#"{"cmd":"session_restore"}"#).expect("restored");
        assert_eq!(
            (restored["restored"].clone(), restored["count"].clone(), restored["active"].clone()),
            (true.into(), 2.into(), 2.into()),
            "{restored}"
        );
        let refused = run(&host, Level::ReadOnly, r#"{"cmd":"session_save"}"#).expect_err("saving is doing");
        assert_eq!(refused.code, "permission_denied");
    }

    /// **Failure (R3.5a): a windowless session's `session save` counts lists in memory as `edited_tabs`,** so a tab whose every step was undone, with
    /// nothing unsaved, is counted as edited. It's the tabs with unsaved changes, by the read the close and quit rules use, as the app counts them.
    #[test]
    fn a_save_counts_the_tabs_with_unsaved_changes() {
        let (host, _root) = with_record();
        for name in ["a", "b"] {
            run(&host, Level::Full, &format!(r#"{{"cmd":"open","path":"/d/{name}.pdf"}}"#)).expect("opened");
        }
        for line in [
            r##"{"cmd":"page_rotate","tab":"#1","pages":"1","degrees":90}"##,
            r##"{"cmd":"page_rotate","tab":"#2","pages":"1","degrees":90}"##,
            r##"{"cmd":"undo","tab":"#2"}"##,
        ] {
            run(&host, Level::Full, line).expect(line);
        }
        let saved = run(&host, Level::Control, r#"{"cmd":"session_save"}"#).expect("saved");
        assert_eq!(saved["edited_tabs"], 1, "only #1 has unsaved changes: {saved}");
    }

    /// Failure: a long-running session that keeps no record answers one of the record's commands as if it did, or as needing the long-running session it is, or says so before it says the level is too low.
    #[test]
    fn with_no_record_each_record_command_says_so() {
        let host = SessionHost::new(Fake);
        run(&host, Level::Full, r#"{"cmd":"open","path":"/d/a.pdf"}"#).expect("opened");
        for line in [
            r#"{"cmd":"session_show"}"#,
            r#"{"cmd":"session_sleep"}"#,
            r#"{"cmd":"session_save"}"#,
            r#"{"cmd":"session_restore"}"#,
            r#"{"cmd":"session_check"}"#,
        ] {
            assert_eq!(run(&host, Level::Full, line).expect_err(line).code, "no_record", "{line}");
        }
        assert_eq!(
            run(&host, Level::ReadOnly, r#"{"cmd":"session_save"}"#).expect_err("refused").code,
            "permission_denied"
        );
    }

    /// Failure: `session restore` with nothing to restore answers in another shape here than in the app, leaving out which tab is active, so an agent reading `active` after a restore gets nothing on one host and a tab on the other. Or it loses the tabs that are open, or says it restored something. (The one change of R2.2's record commands: both hosts now give one shape.)
    #[test]
    fn a_restore_with_nothing_kept_says_the_active_tab_as_the_app_does() {
        let (host, _root) = with_record();
        run(&host, Level::Full, r#"{"cmd":"open","path":"/d/a.pdf"}"#).expect("opened");
        let restored = run(&host, Level::Control, r#"{"cmd":"session_restore"}"#).expect("answered");
        assert_eq!(
            (restored["restored"].clone(), restored["count"].clone(), restored["active"].clone()),
            (false.into(), 1.into(), 1.into()),
            "{restored}"
        );
    }
}
