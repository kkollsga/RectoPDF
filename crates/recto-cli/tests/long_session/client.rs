//! The long workload's transport, using the same CLI parser, checked public client and server executor. The opt-in CLI mode exists only for matched before/after measurements.

use crate::common::{app::Run, evidence};
use recto_control::client::Client;
use serde_json::Value;
use std::cell::Cell;
use std::time::Duration;

#[derive(Clone, Copy, Default)]
pub(super) struct Counts {
    pub cli: usize,
    pub protocol: usize,
    pub sysctl: usize,
}

pub(super) struct Workload<'a, 'state> {
    run: &'a Run<'state>,
    cli: bool,
    represented: Cell<u8>,
    counts: Cell<Counts>,
    progress: Cell<(&'static str, usize, usize)>,
}

impl<'a, 'state> Workload<'a, 'state> {
    pub fn new(run: &'a Run<'state>) -> Self {
        Self {
            run,
            cli: std::env::var("RECTO_LONG_SESSION_CLIENT").as_deref() == Ok("cli"),
            represented: Cell::new(0),
            counts: Cell::new(Counts::default()),
            progress: Cell::new(("opening", 0, 0)),
        }
    }

    pub fn cli_baseline(&self) -> bool {
        self.cli
    }

    pub fn at(&self, phase: &'static str, round: usize, row: usize) {
        self.progress.set((phase, round, row));
    }

    pub fn counts(&self) -> Counts {
        self.counts.get()
    }

    pub fn ok(&self, args: &[&str]) -> Value {
        let (phase, round, row) = self.progress.get();
        eprintln!("long-session progress: phase={phase} round={round} row={row} command={args:?}");
        let bit = match args {
            ["open", _] => 1,
            ["qa", "wait-settled", ..] => 2,
            ["qa", "state"] => 4,
            _ => 0,
        };
        if self.cli || (bit != 0 && self.represented.get() & bit == 0) {
            let answer = self.run.ok(args);
            self.represented.set(self.represented.get() | bit);
            self.counts.set(Counts {
                cli: self.counts.get().cli + 1,
                ..self.counts.get()
            });
            return answer;
        }
        let words: Vec<String> = args.iter().map(|word| (*word).to_owned()).collect();
        let invocation = recto_agent::table::parse_cli(&words).unwrap_or_else(|error| panic!("{args:?}: {error:?}"));
        invocation.command().unwrap_or_else(|error| panic!("{args:?}: {error:?}"));
        // These workload commands use only explicit QA timeouts. This is the CLI's spec budget plus that same parsed timeout, never a larger test-only wait.
        let timeout_ms = invocation.fields.get("timeout_ms").and_then(Value::as_u64).unwrap_or(0);
        let wait = Duration::from_secs(invocation.spec.client_wait_s) + Duration::from_millis(timeout_ms);
        // Match recto's one connection per command. Never retry an edit whose result might already have taken effect.
        let answer = Client::connect(&self.run.state.paths)
            .and_then(|mut client| client.call(&invocation.wire(), wait))
            .unwrap_or_else(|error| {
                panic!(
                    "{args:?}: {error:?}\n{}",
                    crate::common::app::what_became_of(&self.run.state, Some(self.run.pid))
                )
            });
        assert_eq!(answer["ok"], true, "{args:?}: {answer}{}", self.run.if_gone(&answer));
        if args == ["qa", "state"] {
            evidence::read_state(self.run.pid, &answer["data"]);
        }
        self.counts.set(Counts {
            protocol: self.counts.get().protocol + 1,
            ..self.counts.get()
        });
        answer["data"].clone()
    }

    /// A snapshot at a labelled point, not a command for every wake. Baseline mode retains the old sampling cadence so the process-count comparison is measured.
    pub fn load(&self) -> String {
        let output = recto_platform::test_command::output(
            std::process::Command::new("/usr/sbin/sysctl").args(["-n", "vm.loadavg"]),
            Duration::from_secs(5),
        );
        match output {
            Ok((exceeded, output)) => {
                self.counts.set(Counts {
                    sysctl: self.counts.get().sysctl + 1,
                    ..self.counts.get()
                });
                if exceeded || !output.status.success() {
                    format!(
                        "unreadable: {:?}, bound_exceeded={exceeded}, {}",
                        output.status,
                        String::from_utf8_lossy(&output.stderr)
                    )
                } else {
                    String::from_utf8_lossy(&output.stdout).trim().to_owned()
                }
            }
            Err(error) => format!("unreadable: {error}"),
        }
    }
}

impl Drop for Workload<'_, '_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("long-session load at failure: {}", self.load());
        }
    }
}
