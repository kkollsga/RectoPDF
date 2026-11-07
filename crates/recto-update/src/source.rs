//! Which server, and how far this build may go. **A release build reads no override.** The checks below all start from [`crate::RELEASE_BUILD`], which is compiled in, so nothing in the environment can point a release at another server or loosen what it accepts.

use crate::{REPO, RELEASE_BUILD, TEAM_ID};

/// Points a test or development build at a loopback server. Read only in a build that isn't a release, on a state folder set on purpose.
pub const REPO_ENV: &str = "RECTO_UPDATE_REPO";
/// `developer-id` makes a test run hold a download to the real rule, to test the refusal.
pub const TRUST_ENV: &str = "RECTO_UPDATE_TRUST";

/// What a download has to prove.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trust {
    /// Signed by this team and accepted by Gatekeeper. The only one a release build has.
    DeveloperId { team: &'static str },
    /// Signed, by anyone. A test's own ad-hoc build. Only a test run.
    AnySignature,
}

/// How far a build goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Checks every day and replaces itself.
    Release,
    /// Checks only when asked, and never replaces itself.
    Development,
    /// Behaves as a release, against a test's own loopback server, on temp state.
    Test,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub mode: Mode,
    pub repo: String,
    pub trust: Trust,
    /// May reach the server at all. A quiet run on temp state never reaches the real one, in any kind of build.
    pub network: bool,
    /// The file's own override, for the test to carry across to the new version.
    pub test_env: Vec<(String, String)>,
}

/// A test's own server: plain HTTP to this very machine, on a port, and nothing else.
fn is_loopback(url: &str) -> bool {
    url.strip_prefix("http://127.0.0.1:").is_some_and(|port| !port.is_empty() && port.len() <= 5 && port.bytes().all(|byte| byte.is_ascii_digit()))
}

/// The rule itself, with everything it depends on given. `state_overridden`: running on a state folder set on purpose, not the user's.
#[must_use]
pub fn resolve(release_build: bool, state_overridden: bool, env_repo: Option<String>, env_trust: Option<String>) -> Source {
    let real = Trust::DeveloperId { team: TEAM_ID };
    if release_build {
        return Source { mode: Mode::Release, repo: REPO.to_owned(), trust: real, network: !state_overridden, test_env: Vec::new() };
    }
    if state_overridden && let Some(repo) = env_repo.filter(|repo| is_loopback(repo)) {
        let trust = if env_trust.as_deref() == Some("developer-id") { real } else { Trust::AnySignature };
        let mut test_env = vec![(REPO_ENV.to_owned(), repo.clone())];
        if let Some(value) = env_trust {
            test_env.push((TRUST_ENV.to_owned(), value));
        }
        return Source { mode: Mode::Test, repo, trust, network: true, test_env };
    }
    Source { mode: Mode::Development, repo: REPO.to_owned(), trust: real, network: !state_overridden, test_env: Vec::new() }
}

/// This build's source. A release build doesn't look at the environment.
#[must_use]
pub fn from_env(state_overridden: bool) -> Source {
    if RELEASE_BUILD {
        return resolve(true, state_overridden, None, None);
    }
    let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    resolve(false, state_overridden, var(REPO_ENV), var(TRUST_ENV))
}

impl Source {
    /// Checks by itself every day, whichever way the switch is set.
    #[must_use]
    pub fn checks_by_itself(&self) -> bool {
        self.network && matches!(self.mode, Mode::Release | Mode::Test)
    }

    /// Replaces itself. A development build never does.
    #[must_use]
    pub fn may_replace_itself(&self) -> bool {
        self.network && matches!(self.mode, Mode::Release | Mode::Test)
    }

    /// The switch's position until someone sets it: on for a release build. Owner, 2026-11-07.
    #[must_use]
    pub fn auto_default(&self) -> bool {
        matches!(self.mode, Mode::Release | Mode::Test)
    }

    #[must_use]
    pub fn mode_name(&self) -> &'static str {
        match self.mode {
            Mode::Release => "release",
            Mode::Development => "development",
            Mode::Test => "test",
        }
    }

    #[must_use]
    pub fn trust_name(&self) -> &'static str {
        match self.trust {
            Trust::DeveloperId { .. } => "developer_id",
            Trust::AnySignature => "any_signature",
        }
    }

    /// Where the release's description is.
    #[must_use]
    pub fn latest_url(&self) -> String {
        format!("{}/releases/latest/download/latest.json", self.repo.trim_end_matches('/'))
    }

    /// Where a version's zip is. Only ever made of a checked version.
    #[must_use]
    pub fn zip_url(&self, version: &str) -> String {
        format!("{}/releases/download/v{version}/{}", self.repo.trim_end_matches('/'), crate::rules::zip_name(version))
    }

    /// Whether a plain `http://` address may be followed. Only to a test's own server.
    #[must_use]
    pub fn allows_plain_http(&self) -> bool {
        self.mode == Mode::Test
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOOPBACK: &str = "http://127.0.0.1:7199";

    /// Failure: an override reaches a release build, by the environment or by a state folder set on purpose, so someone who can set a variable can point the app at their server or loosen what it accepts. It's the rule with everything given, and `from_env` below is how a build gets to it.
    #[test]
    fn a_release_build_reads_no_override() {
        for overridden in [false, true] {
            let source = resolve(true, overridden, Some(LOOPBACK.into()), Some("any".into()));
            assert_eq!(source.mode, Mode::Release);
            assert_eq!(source.repo, REPO);
            assert_eq!(source.trust, Trust::DeveloperId { team: TEAM_ID });
            assert!(source.test_env.is_empty());
            assert!(!source.allows_plain_http());
        }
        assert!(!resolve(true, true, None, None).network, "a quiet run reaches no server, in any build");
        assert!(resolve(true, false, None, None).checks_by_itself());
        assert!(resolve(true, false, None, None).may_replace_itself());
        assert!(resolve(true, false, None, None).auto_default());
    }

    /// Failure: this build's `from_env` ignores its compiled kind. Run with `RECTO_RELEASE=1` set at compile time, as `app/release-zip.sh` does, the variables below must change nothing: the release check there runs the real binary the same way.
    #[test]
    fn from_env_follows_what_was_compiled() {
        let source = from_env(true);
        if RELEASE_BUILD {
            assert_eq!(source.mode, Mode::Release);
            assert_eq!(source.repo, REPO);
        } else {
            assert_ne!(source.mode, Mode::Release);
        }
    }

    /// Failure: a development build replaces itself or checks by itself, a test override works on the user's real state, or a test can name a server other than this machine.
    #[test]
    fn a_development_build_only_checks_when_asked_and_a_test_stays_on_this_machine() {
        let plain = resolve(false, false, None, None);
        assert_eq!(plain.mode, Mode::Development);
        assert!(plain.network, "it can check when asked");
        assert!(!plain.checks_by_itself());
        assert!(!plain.may_replace_itself());
        assert!(!plain.auto_default());
        assert_eq!(resolve(false, false, Some(LOOPBACK.into()), None).mode, Mode::Development, "on the user's real state, the override is nothing");
        let test = resolve(false, true, Some(LOOPBACK.into()), None);
        assert_eq!((test.mode, test.trust), (Mode::Test, Trust::AnySignature));
        assert!(test.checks_by_itself() && test.may_replace_itself());
        assert!(test.allows_plain_http());
        assert_eq!(resolve(false, true, Some(LOOPBACK.into()), Some("developer-id".into())).trust, Trust::DeveloperId { team: TEAM_ID }, "a test of the refusal");
        for elsewhere in ["https://evil.invalid", "http://evil.invalid:80", "http://127.0.0.1.evil.invalid:80", "http://127.0.0.1:80/../x", "http://127.0.0.1:", "file:///etc", "http://localhost:80"] {
            let source = resolve(false, true, Some(elsewhere.into()), None);
            assert_eq!(source.mode, Mode::Development, "{elsewhere}");
            assert!(!source.network, "a quiet run reaches no real server");
        }
    }
}
