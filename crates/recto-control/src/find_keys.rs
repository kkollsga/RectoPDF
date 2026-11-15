//! **Which Find key a session seals with** (owner, 2026-11-15, "Lets store it encrypted"; the coordinator's decisions on the design, the same day). Decided once per session, from its state folder and who hosts it, and taken the first time an index is used, on the thread asking, never the UI thread:
//! - a test state folder (`RECTO_STATE_DIR`): its own file, `find.key`, whoever hosts it, so quiet runs and tests never reach the Keychain;
//! - the app on the person's own state: the Keychain if this copy is the team's, else a key in memory, by the loader only the app passes in ([`AppKey`]), so no other program even links the Keychain's code;
//! - `recto` hosting a session of its own on the person's own state: no key, so it keeps no index and reads pages, as on a volume that can't be indexed. Talking to the app's session, it uses the app's.

use crate::state::StatePaths;
use recto_platform::find_key::FindKey;
use serde_json::{Value, json};
use std::path::PathBuf;

/// How the app takes its key: `recto_platform::find_key::load_for_the_app` with the team, passed in by the app's own start and by nothing else.
pub type AppKey = fn() -> Option<(FindKey, Option<&'static str>)>;

/// Who's hosting the session.
#[derive(Clone, Copy, Debug)]
pub enum Host {
    /// The app's own process, with its way to the Keychain.
    App(AppKey),
    /// `recto serve`, `recto local` and the like.
    Cli,
}

/// Where the key will come from.
#[derive(Clone, Debug)]
pub enum Plan {
    StateFile(PathBuf),
    AppKeychain(AppKey),
    /// A key for this run alone: a host with nothing to keep, such as a test's.
    Memory,
    /// Nothing's indexed: this is why, as a code for `index.note`.
    NoKey(&'static str),
}

impl Plan {
    /// What the plan is, in a word, for `qa state` and `index status`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::StateFile(_) => "state_file",
            Self::AppKeychain(_) => "keychain",
            Self::Memory => "memory",
            Self::NoKey(_) => "none",
        }
    }
}

/// The plan for a session on these folders. A real run never gets a file key: only a state folder named by the override can.
pub fn plan(paths: &StatePaths, host: Host) -> Plan {
    match (paths.overridden, host) {
        (true, _) => Plan::StateFile(paths.root.join("find.key")),
        (false, Host::App(load)) => Plan::AppKeychain(load),
        (false, Host::Cli) => Plan::NoKey("not_the_app"),
    }
}

/// A key as it was had: the key, and why it's in memory when that isn't what the plan wanted.
#[derive(Debug)]
pub struct Loaded {
    pub key: Option<FindKey>,
    pub why: Option<&'static str>,
}

/// What `qa state` and `index status` say of the key: the plan, and once it's been taken, where it came from and why that isn't the plan's own. Never anything of the key.
pub fn report(plan: &Plan, loaded: Option<&Loaded>) -> Value {
    let why = match (plan, loaded) {
        (Plan::NoKey(why), _) => Some(*why),
        (_, Some(loaded)) => loaded.why,
        (_, None) => None,
    };
    json!({
        "plan": plan.code(),
        "source": loaded.and_then(|loaded| loaded.key.as_ref()).map(|key| key.source().code()),
        "why": why,
    })
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
        Plan::AppKeychain(load) => match load() {
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
        fn never() -> Option<(FindKey, Option<&'static str>)> {
            panic!("a test reached the app's Keychain loader")
        }
        let app = Host::App(never);
        let in_file = |plan: Plan| matches!(plan, Plan::StateFile(path) if path == quiet.root.join("find.key"));
        assert!(in_file(plan(&quiet, app)) && in_file(plan(&quiet, Host::Cli)));
        let real = StatePaths::users_own().unwrap();
        assert!(matches!(plan(&real, app), Plan::AppKeychain(_)));
        assert!(matches!(plan(&real, Host::Cli), Plan::NoKey("not_the_app")));
        let none = load(&plan(&real, Host::Cli));
        assert!(none.key.is_none());
        assert_eq!(
            report(&plan(&real, Host::Cli), Some(&none)),
            json!({ "plan": "none", "source": null, "why": "not_the_app" })
        );
        assert_eq!(
            report(&plan(&real, app), None),
            json!({ "plan": "keychain", "source": null, "why": null }),
            "said before it's been taken, and without taking it"
        );
        let taken = load(&plan(&quiet, Host::Cli));
        assert_eq!(
            report(&plan(&quiet, Host::Cli), Some(&taken)),
            json!({ "plan": "state_file", "source": "state_file", "why": null })
        );
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
        // By file: the app's own start, `load_for_the_app` for the Keychain, and this test's own words.
        let mut files: Vec<String> = callers.iter().map(|at| at.rsplit_once(':').unwrap().0.to_string()).collect();
        files.sort();
        files.dedup();
        assert_eq!(
            files,
            [
                "recto-app/src/core/start.rs",
                "recto-control/src/find_keys.rs",
                "recto-platform/src/find_key.rs"
            ],
            "the Keychain is reached from somewhere new: {callers:?}"
        );
    }
}
