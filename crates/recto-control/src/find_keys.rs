//! **Which Find key a session seals with** (owner, 2026-11-15, "Lets store it encrypted"; the coordinator's decisions on the design, the same day). Decided once per session, from its state folder and who hosts it, and taken the first time an index is used, on the thread asking, never the UI thread:
//! - a test state folder (`RECTO_STATE_DIR`): its own file, `find.key`, whoever hosts it, so quiet runs and tests never reach the Keychain;
//! - the app on the person's own state: the Keychain if this copy is the team's, else a key in memory ([`recto_platform::find_key::load_for_the_app`]);
//! - `recto` hosting a session of its own on the person's own state: no key, so it keeps no index and reads pages, as on a volume that can't be indexed. Talking to the app's session, it uses the app's.

use crate::state::StatePaths;
use recto_platform::find_key::FindKey;
use std::path::PathBuf;

/// Who's hosting the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    /// The app's own process, signed by `team` if it's a release.
    App { team: &'static str },
    /// `recto serve`, `recto local` and the like.
    Cli,
}

/// Where the key will come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    StateFile(PathBuf),
    AppKeychain {
        team: &'static str,
    },
    /// A key for this run alone: a host with nothing to keep, such as a test's.
    Memory,
    /// Nothing's indexed: this is why, as a code for `index.note`.
    NoKey(&'static str),
}

/// The plan for a session on these folders. A real run never gets a file key: only a state folder named by the override can.
pub fn plan(paths: &StatePaths, host: Host) -> Plan {
    match (paths.overridden, host) {
        (true, _) => Plan::StateFile(paths.root.join("find.key")),
        (false, Host::App { team }) => Plan::AppKeychain { team },
        (false, Host::Cli) => Plan::NoKey("not_the_app"),
    }
}

/// A key as it was had: the key, and why it's in memory when that isn't what the plan wanted.
#[derive(Debug)]
pub struct Loaded {
    pub key: Option<FindKey>,
    pub why: Option<&'static str>,
}

/// Takes the key the plan says. Blocks for one Keychain call at most, so never on the UI thread.
pub fn load(plan: &Plan) -> Loaded {
    match plan {
        Plan::StateFile(path) => match FindKey::from_state_file(path) {
            Ok(key) => Loaded { key: Some(key), why: None },
            Err(_) => Loaded {
                key: FindKey::in_memory().ok(),
                why: Some("state_file_unreadable"),
            },
        },
        Plan::AppKeychain { team } => match recto_platform::find_key::load_for_the_app(team) {
            Some((key, why)) => Loaded { key: Some(key), why },
            None => Loaded {
                key: None,
                why: Some("no_randomness"),
            },
        },
        Plan::Memory => Loaded {
            key: FindKey::in_memory().ok(),
            why: None,
        },
        Plan::NoKey(why) => Loaded { key: None, why: Some(why) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a quiet run or a test reaches the person's Keychain, a real run takes a key from a file anyone could have put there, or `recto` on the real state gets a key at all.
    #[test]
    fn only_the_apps_real_run_asks_the_keychain() {
        let temp = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-find-keys-");
        let quiet = StatePaths::overridden(&temp).unwrap();
        let app = Host::App { team: "6GND8Z2C7Z" };
        assert_eq!(plan(&quiet, app), Plan::StateFile(quiet.root.join("find.key")));
        assert_eq!(plan(&quiet, Host::Cli), Plan::StateFile(quiet.root.join("find.key")));
        let real = StatePaths::users_own().unwrap();
        assert_eq!(plan(&real, app), Plan::AppKeychain { team: "6GND8Z2C7Z" });
        assert_eq!(plan(&real, Host::Cli), Plan::NoKey("not_the_app"));
        let none = load(&plan(&real, Host::Cli));
        assert!(none.key.is_none());
    }

    /// Failure: the Keychain code is reached from somewhere other than this one plan, so a helper, `recto` or a test could ask it. The source is held to one caller.
    #[test]
    fn the_keychain_has_one_caller() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let mut callers = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if !path.ends_with("target") {
                        stack.push(path);
                    }
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap_or_default();
                    for (number, line) in text.lines().enumerate() {
                        if (line.contains("load_for_the_app(") || line.contains("keychain::load_or_create("))
                            && !line.trim_start().starts_with("//")
                            && !line.contains("pub fn ")
                        {
                            callers.push(format!("{}:{}", path.strip_prefix(root).unwrap().display(), number + 1));
                        }
                    }
                }
            }
        }
        // By file: this plan for the app (and this test's own words), and `load_for_the_app` for the Keychain.
        let mut files: Vec<String> = callers.iter().map(|at| at.rsplit_once(':').unwrap().0.to_string()).collect();
        files.sort();
        files.dedup();
        assert_eq!(
            files,
            ["recto-control/src/find_keys.rs", "recto-platform/src/find_key.rs"],
            "the Keychain is reached from somewhere new: {callers:?}"
        );
    }
}
