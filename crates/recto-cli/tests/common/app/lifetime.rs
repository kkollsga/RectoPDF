//! A launched app is an owned Child; an attached app is only its original control connection.
use recto_control::{StatePaths, client::Client};
use serde_json::Value;
use std::process::Child;
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(5);

pub enum Lifetime {
    Owned(Child),
    Borrowed(Option<Client>),
}

fn call(client: &mut Client, line: &str) -> Result<Value, String> {
    let reply = client.call(line, WAIT).map_err(|error| format!("{}: {}", error.code, error.message))?;
    if reply["ok"] == true {
        Ok(reply["data"].clone())
    } else {
        Err(reply.to_string())
    }
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

    /// Status and stop share one connection. An owned unreaped Child anchors its PID while identity is checked.
    pub fn stop(&mut self, paths: &StatePaths) -> Result<(), String> {
        match self {
            Self::Owned(child) => {
                if child.try_wait().map_err(|error| error.to_string())?.is_some() {
                    return Ok(());
                }
                let mut client = matching(paths, child.id())?;
                call(&mut client, r#"{"cmd":"stop"}"#).map(|_| ())
            }
            Self::Borrowed(Some(client)) => call(client, r#"{"cmd":"stop"}"#).map(|_| ()),
            Self::Borrowed(None) => Err("the original attached connection is unavailable; no reconnect or PID signal was attempted".into()),
        }
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
