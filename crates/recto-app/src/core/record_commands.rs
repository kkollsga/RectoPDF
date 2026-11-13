//! **The app's state record, for the record's commands** (`session show`, `sleep`, `save`, `restore`, `check`). What each answers is `recto-agent`'s, in `commands/record.rs`, which the windowless session shares; this is only what the app does for them: its record kept as it goes, its sleeper, its saver's timings and its real look at whether a hand-off could go. Registered once, in `start.rs`, on the session the socket and the window share.
//!
//! Holds the app's host weakly, as the host holds the session that holds this. Locks: each call takes the session's lock only for a look of its own, after its work, and never holds it across a push to the session thread or a wait for it (`flush`): the session thread takes that lock itself. The notice's lock is taken and let go before anything else.

use super::*;
use recto_agent::commands::record::{HandOff, Records, Restored, SaveTimings, Saved, Slept};
use std::sync::Weak;

/// The app's record, reached through its host.
pub struct AppRecords(pub Weak<AppHost>);

impl AppRecords {
    fn host(&self) -> Result<Arc<AppHost>, AgentError> {
        self.0
            .upgrade()
            .ok_or_else(|| AgentError::new("busy", "The app is going away.", recto_agent::exit::BUSY))
    }
}

impl Records for AppRecords {
    fn kept(&self) -> Value {
        let Ok(host) = self.host() else { return Value::Null };
        let notice = lock(&host.notice).clone();
        kept_json(&host.saver.store, notice.as_deref().and_then(record::notices::words))
    }

    /// Through the sleeper, as five minutes left alone would: its pictures and its journal too.
    fn put_to_sleep(&self, tab: TabId) -> Result<Slept, AgentError> {
        let host = self.host()?;
        host.core.push(CoreCall::SleepTabs(vec![tab], sleeper::Why::Asked));
        host.core.flush(Duration::from_secs(5));
        Ok(host.session.with_session(|session| Slept {
            dormant: !session.warm_tabs().any(|other| other == tab),
            list_asleep: session.list_is_asleep(tab),
        }))
    }

    fn save(&self) -> Result<Saved, AgentError> {
        let report = self.host()?.saver.flush(false, Duration::from_secs(5)).map_err(store_error)?;
        Ok(Saved {
            tabs: report.tabs,
            edited_tabs: report.edited_tabs,
            record_bytes: report.record_bytes,
            journal_bytes: report.journal_bytes,
            wrote_bytes: report.wrote_bytes,
            total_us: report.total_us,
            timings: Some(SaveTimings {
                write_us: report.write_us,
                force_us: report.force_us,
                verify_us: report.verify_us,
            }),
        })
    }

    /// Taken in on the session thread, as a launch's is, whatever was found: nothing found is the session as it was.
    fn restore(&self) -> Result<Restored, AgentError> {
        let host = self.host()?;
        let loaded = host.saver.store.load();
        let notice = notice_for(&loaded).and_then(record::notices::words);
        let restored = matches!(loaded, Loaded::Restored(_));
        host.core.push(CoreCall::Restored(Box::new(loaded)));
        host.core.flush(Duration::from_secs(5));
        Ok(Restored { restored, notice })
    }

    fn hand_off(&self) -> HandOff {
        let Ok(host) = self.host() else {
            return HandOff {
                safe: false,
                busy: Vec::new(),
                seconds_since_input: Some(0.0),
            };
        };
        let check = hand_off_check(&host);
        HandOff {
            safe: check.safe,
            busy: check.busy.into_iter().map(|item| (item.code, item.message)).collect(),
            seconds_since_input: Some(check.seconds_since_input),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::testkit::headless::HeadlessApp;
    use recto_agent::{AgentError, Level, parse};
    use recto_control::host::Host;
    use serde_json::Value;

    fn run(app: &HeadlessApp, level: Level, line: &str) -> Result<Value, AgentError> {
        app.core()
            .host
            .run(recto_agent::Actor::Agent(level), &parse(line).expect("a command"), &mut Vec::new())
    }

    /// Failure: the app answers the record's commands otherwise than it did when it answered them itself, now that `recto-agent` makes the answers it shares with the session with no window: its hand-off check is the windowless "any moment's safe" in place of its own look, a tab asked to sleep doesn't go through its sleeper, a save loses its timings, or a restore loses the active tab. Or the level isn't checked on this path, which only the server checked before.
    #[test]
    fn the_app_answers_the_record_commands_from_its_own_record() {
        let app = HeadlessApp::start();
        let (a, b) = (app.disk().pdf("a.pdf", 3), app.disk().pdf("b.pdf", 3));
        app.open(&a);
        app.open(&b);
        app.settle();
        let check = run(&app, Level::ReadOnly, r#"{"cmd":"session_check"}"#).expect("check");
        assert!(check["safe"].is_boolean() && check["busy"].is_array(), "{check}");
        assert!(
            check["seconds_since_input"].is_f64(),
            "the app's own look says how long since the user touched it: {check}"
        );
        let show = run(&app, Level::ReadOnly, r#"{"cmd":"session_show"}"#).expect("show");
        assert_eq!(
            (show["record"]["holds_the_record"].clone(), show["count"].clone()),
            (true.into(), 2.into()),
            "{show}"
        );
        let slept = run(&app, Level::Control, r##"{"cmd":"session_sleep","tab":"#1"}"##).expect("slept");
        assert_eq!(
            (slept["tab"].clone(), slept["dormant"].clone()),
            (1.into(), true.into()),
            "through the sleeper: {slept}"
        );
        let saved = run(&app, Level::Control, r#"{"cmd":"session_save"}"#).expect("saved");
        assert_eq!((saved["saved"].clone(), saved["tabs"].clone()), (true.into(), 2.into()), "{saved}");
        for timing in ["total_us", "write_us", "force_us", "verify_us"] {
            assert!(saved[timing].is_u64(), "{timing}: {saved}");
        }
        assert_eq!(saved["total_ms"].as_u64(), saved["total_us"].as_u64().map(|us| us / 1000), "{saved}");
        let restored = run(&app, Level::Control, r#"{"cmd":"session_restore"}"#).expect("restored");
        assert_eq!(
            (restored["restored"].clone(), restored["count"].clone(), restored["active"].clone()),
            (true.into(), 2.into(), 2.into()),
            "{restored}"
        );
        assert_eq!(
            run(&app, Level::ReadOnly, r#"{"cmd":"session_save"}"#).expect_err("saving is doing").code,
            "permission_denied"
        );
    }
}
