//! A launched app is an owned Child; an attached app is only its original control connection.
use recto_control::{StatePaths, client::Client};
use serde_json::Value;
use std::process::Child;
use std::time::{Duration, Instant};

/// How long a quiet run lasts unless a test asks for longer: the harness's own limit on a test (`.config/nextest.toml`: 60 s a period, ended after 5), so the run is never the first to end mid-test (2026-11-26: a run's own 120 s ended a test the harness still allowed). `--owner` ends the run when the test ends, so a long limit costs nothing.
pub const TEST_LIMIT: Duration = Duration::from_secs(300);

/// The `--timeout` a test's `qa wait-settled` is given: **a hang guard, not a speed bound.** The app's own default is 30 s of wall clock, which a starved machine (load 15 to 29) runs out while the app is still working (3 of 30 lifecycle runs, 2026-11-27). A settle that never comes is a hang, and this ends it with a verdict; it must stay under [`TEST_LIMIT`] so the test, not the run, reports it.
pub const SETTLE_GUARD: Duration = Duration::from_secs(120);

/// `--timeout`'s argument for [`SETTLE_GUARD`].
#[must_use]
pub fn settle_guard_arg() -> String {
    format!("{}s", SETTLE_GUARD.as_secs())
}

const WAIT: Duration = Duration::from_secs(5);

pub enum Lifetime {
    Owned(Child),
    Borrowed(Option<Client>),
}

#[derive(Debug)]
pub(super) enum RequestError {
    SessionEnded,
    Other(String),
}
impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionEnded => f.write_str("session_ended: The session ended before it answered."),
            Self::Other(message) => f.write_str(message),
        }
    }
}

fn call_typed(client: &mut Client, line: &str, wait: Duration) -> Result<Value, RequestError> {
    let reply = client.call(line, wait).map_err(|error| {
        if error.code == "session_ended" {
            RequestError::SessionEnded
        } else {
            RequestError::Other(format!("{}: {}", error.code, error.message))
        }
    })?;
    if reply["ok"] == true {
        Ok(reply["data"].clone())
    } else {
        Err(RequestError::Other(reply.to_string()))
    }
}

fn call_for(client: &mut Client, line: &str, wait: Duration) -> Result<Value, String> {
    call_typed(client, line, wait).map_err(|error| error.to_string())
}

pub(super) fn finish_terminal(reply: Result<Value, RequestError>, proof: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    match reply {
        Ok(_) | Err(RequestError::SessionEnded) => proof(),
        Err(error) => Err(error.to_string()),
    }
}

fn call(client: &mut Client, line: &str) -> Result<Value, String> {
    call_for(client, line, WAIT)
}

fn matching(paths: &StatePaths, pid: u32) -> Result<Client, String> {
    let mut client = Client::connect(paths).map_err(|error| format!("{}: {}", error.code, error.message))?;
    let status = call(&mut client, r#"{"cmd":"status"}"#)?;
    if status["pid"].as_u64() != Some(u64::from(pid)) {
        return Err(format!("the connected app isn't the expected process {pid}: {status}"));
    }
    Ok(client)
}

impl Lifetime {
    pub fn attached(paths: &StatePaths, pid: u32) -> Self {
        match matching(paths, pid) {
            Ok(client) => Self::Borrowed(Some(client)),
            Err(error) => {
                eprintln!("couldn't retain attached quiet app {pid}: {error}; its existing owner/state lifetime remains in force");
                Self::Borrowed(None)
            }
        }
    }

    /// Lifecycle commands use the retained attached connection, or a connection checked against the owned unreaped Child.
    pub fn request(&mut self, paths: &StatePaths, args: &[&str]) -> Result<Value, String> {
        let words: Vec<String> = args.iter().map(|word| (*word).to_owned()).collect();
        let invocation = recto_agent::table::parse_cli(&words).map_err(|error| format!("{error:?}"))?;
        invocation.command().map_err(|error| format!("{error:?}"))?;
        let wait = Duration::from_secs(invocation.spec.client_wait_s);
        self.request_line(paths, &invocation.wire(), wait)
    }

    fn request_line(&mut self, paths: &StatePaths, line: &str, wait: Duration) -> Result<Value, String> {
        self.request_typed(paths, line, wait).map_err(|error| error.to_string())
    }

    pub(super) fn quit_reply(&mut self, paths: &StatePaths) -> Result<Value, RequestError> {
        let words = ["qa", "quit", "now"].map(str::to_owned);
        let invocation = recto_agent::table::parse_cli(&words).map_err(|error| RequestError::Other(format!("{error:?}")))?;
        invocation.command().map_err(|error| RequestError::Other(format!("{error:?}")))?;
        self.request_typed(paths, &invocation.wire(), Duration::from_secs(invocation.spec.client_wait_s))
    }

    fn request_typed(&mut self, paths: &StatePaths, line: &str, wait: Duration) -> Result<Value, RequestError> {
        match self {
            Self::Owned(child) => {
                if child.try_wait().map_err(|error| RequestError::Other(error.to_string()))?.is_some() {
                    return Err(RequestError::Other("the owned app has already exited".into()));
                }
                let mut client = matching(paths, child.id()).map_err(RequestError::Other)?;
                call_typed(&mut client, line, wait)
            }
            Self::Borrowed(Some(client)) => call_typed(client, line, wait),
            Self::Borrowed(None) => Err(RequestError::Other(
                "the original attached connection is unavailable; no reconnect or PID signal was attempted".into(),
            )),
        }
    }

    /// Status and stop share one connection. An owned unreaped Child anchors its PID while identity is checked.
    pub(super) fn stop_reply(&mut self, paths: &StatePaths) -> Result<Value, RequestError> {
        if let Self::Owned(child) = self
            && child.try_wait().map_err(|error| RequestError::Other(error.to_string()))?.is_some()
        {
            return Ok(Value::Null);
        }
        self.request_typed(paths, r#"{"cmd":"stop"}"#, WAIT)
    }

    pub fn owned(&self) -> bool {
        matches!(self, Self::Owned(_))
    }

    pub fn gone(&mut self, pid: u32) -> Result<bool, String> {
        match self {
            Self::Owned(child) => child.try_wait().map(|status| status.is_some()).map_err(|error| error.to_string()),
            Self::Borrowed(_) => Ok(recto_platform::local::process_counters(pid).is_none()),
        }
    }

    /// SIGTERM's quit semantics, with the PID anchored by our unreaped Child, never an attached numeric PID.
    pub fn terminate_owned(&mut self) -> Result<(), String> {
        let Self::Owned(child) = self else {
            return Err("SIGTERM requires the Child owned by this launch; attached runs cannot be signalled".into());
        };
        if child.try_wait().map_err(|error| error.to_string())?.is_some() {
            return Ok(());
        }
        if !recto_platform::local::signal_process(child.id(), false) {
            return Err(format!("couldn't terminate owned child {}", child.id()));
        }
        Ok(())
    }

    /// Only the unreaped Child can be killed. An attached instance is never signalled.
    pub fn kill_owned(&mut self, wait: Duration) -> Result<(), String> {
        let Self::Owned(child) = self else {
            return Err("intentional crash requires the Child owned by this launch; attached runs cannot be killed".into());
        };
        if child.try_wait().map_err(|error| error.to_string())?.is_some() {
            return Ok(());
        }
        child
            .kill()
            .map_err(|error| format!("owned child {} couldn't be killed: {error}", child.id()))?;
        let began = Instant::now();
        loop {
            if child.try_wait().map_err(|error| error.to_string())?.is_some() {
                return Ok(());
            }
            let Some(left) = wait.checked_sub(began.elapsed()) else {
                return Err(format!("owned child {} still hadn't exited after kill within {wait:?}", child.id()));
            };
            std::thread::sleep(left.min(Duration::from_millis(10))); // ratchet: poll
        }
    }
}

impl Drop for Lifetime {
    fn drop(&mut self) {
        if self.owned()
            && let Err(error) = self.kill_owned(WAIT)
        {
            eprintln!("owned quiet child cleanup: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    /// Failure: shutdown loses its reply although the exact app and socket ended; other failures or missing exit proof must still fail.
    fn a_lost_shutdown_reply_requires_complete_exit_proof() {
        assert!(finish_terminal(Err(RequestError::SessionEnded), || Ok(())).is_ok());
        assert!(finish_terminal(Err(RequestError::SessionEnded), || Err("app or socket remains".into())).is_err());
        assert!(finish_terminal(Err(RequestError::Other("timeout".into())), || Ok(())).is_err());
        assert!(finish_terminal(Ok(Value::Null), || Err("socket remains".into())).is_err());
    }
}
