//! **Versions, release and development.** A release is a plain `X.Y.Z`. A development build is `X.Y.Z.N`: the release it builds on, Cargo.toml's version, which moves only at a release, and a fourth number `tools/dev-build.sh` counts from 1 for each. So `0.1.0` < `0.1.0.1` < `0.1.0.2` < `0.1.1`: a development build is newer than the release it builds on and older than the next. A release never has a fourth part (`crate::rules::parse_version` is the release's own rule), and the fourth number of a development build is never 0, so no version is two ways at once. Pure.

/// A version, ordered. A release's fourth number is 0, which no development build has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub release: (u32, u32, u32),
    pub build: u32,
}

impl Version {
    /// A development build, not a release.
    #[must_use]
    pub fn is_development(self) -> bool {
        self.build > 0
    }
}

/// One number of a version: digits only, nine at most, and no leading zero.
fn number(part: &str) -> Option<u32> {
    let plain = !part.is_empty() && part.len() <= 9 && part.bytes().all(|byte| byte.is_ascii_digit()) && (part == "0" || !part.starts_with('0'));
    plain.then(|| part.parse().ok()).flatten()
}

/// A release's `X.Y.Z` or a development build's `X.Y.Z.N`, with `N` from 1. Nothing else: no `v`, no suffix, no leading zeros, no fifth part.
#[must_use]
pub fn parse(text: &str) -> Option<Version> {
    let parts: Vec<&str> = text.split('.').collect();
    match parts.as_slice() {
        [major, minor, patch] => Some(Version {
            release: (number(major)?, number(minor)?, number(patch)?),
            build: 0,
        }),
        [major, minor, patch, build] => Some(Version {
            release: (number(major)?, number(minor)?, number(patch)?),
            build: number(build).filter(|build| *build > 0)?,
        }),
        _ => None,
    }
}

/// `text` is a development build's version.
#[must_use]
pub fn is_development(text: &str) -> bool {
    parse(text).is_some_and(Version::is_development)
}

/// `new` is a newer version than `current`, and both are versions. A development build counts after the release it builds on and before the next.
#[must_use]
pub fn is_newer(new: &str, current: &str) -> bool {
    matches!((parse(new), parse(current)), (Some(new), Some(current)) if new > current)
}

/// The version a copy of the app runs as: its development version, when its bundle says one that builds on its release, and otherwise the release. `CFBundleShortVersionString` stays the plain release, as the system expects, and a development build adds `RectoDevVersion` beside it, under the same signature.
#[must_use]
pub fn running(short: &str, development: Option<&str>) -> String {
    let release = parse(short).filter(|version| !version.is_development());
    match (release, development.and_then(parse)) {
        (Some(release), Some(dev)) if dev.is_development() && dev.release == release.release => development.unwrap_or(short).to_owned(),
        _ => short.to_owned(),
    }
}

/// The Info.plist key a development build carries its version in.
pub const DEV_VERSION_KEY: &str = "RectoDevVersion";

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a development build sorts before its release or after the next, so the app offers a downgrade as an update or never moves on to the next release. The owner's numbering: `X.Y.Z` < `X.Y.Z.N` < the next `X.Y.Z`.
    #[test]
    fn a_development_build_sorts_after_its_release_and_before_the_next() {
        let order = ["0.1.0", "0.1.0.1", "0.1.0.2", "0.1.0.10", "0.1.1", "0.1.1.1", "0.2.0", "1.0.0"];
        for (at, earlier) in order.iter().enumerate() {
            for later in &order[at + 1..] {
                assert!(is_newer(later, earlier), "{later} after {earlier}");
                assert!(!is_newer(earlier, later), "{earlier} not after {later}");
            }
            assert!(!is_newer(earlier, earlier), "not newer than itself");
        }
        assert!(is_development("0.1.0.3"));
        assert!(!is_development("0.1.0"));
    }

    /// Failure: something that isn't a version gets through as one: a fourth number of 0 (which would be the release under another name), a fifth part, a suffix or a leading zero. Each could make one build look like another.
    #[test]
    fn only_a_release_or_a_development_build_is_a_version() {
        for bad in [
            "",
            "1",
            "1.2",
            "0.1.0.0",
            "0.1.0.01",
            "0.1.0.1.1",
            "v0.1.0.1",
            "0.1.0.1-beta",
            "0.1.0.x",
            "0.1.0.",
            ".0.1.0",
            "0.1.0.9999999999",
            "01.1.0.1",
            " 0.1.0.1",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
        assert_eq!(parse("0.1.0.7"), Some(Version { release: (0, 1, 0), build: 7 }));
        assert!(!is_newer("garbage", "0.1.0"));
        assert!(!is_newer("0.1.0.1", "garbage"), "an app that can't say what it is takes nothing");
    }

    /// Failure: the app runs as a development version its bundle doesn't build on, as a stale or tampered key would make it, or ignores the one it has.
    #[test]
    fn a_copy_runs_as_its_development_version_only_on_its_own_release() {
        assert_eq!(running("0.1.0", Some("0.1.0.4")), "0.1.0.4");
        assert_eq!(running("0.1.0", None), "0.1.0");
        assert_eq!(running("0.1.0", Some("0.1.1.4")), "0.1.0", "built on another release");
        assert_eq!(running("0.1.0", Some("0.1.0")), "0.1.0");
        assert_eq!(running("0.1.0", Some("rubbish")), "0.1.0");
        assert_eq!(
            running("0.1.0.4", Some("0.1.0.4")),
            "0.1.0.4",
            "a short version that isn't a release isn't made into one"
        );
    }
}
