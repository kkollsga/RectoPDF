//! **What a launch of the app was asked for:** the one parser of its arguments, so whether a launch is quiet, decided before the toolkit does anything, and what the rest
//! of the launch does can't disagree.

use recto_control::StatePaths;
use std::path::PathBuf;
use std::time::Duration;

/// What a launch was asked for.
#[derive(Debug, Default, PartialEq)]
pub struct LaunchArgs {
    pub qa: bool,
    pub memory_limit_mib: Option<u64>,
    pub time_limit: Option<Duration>,
    /// The process a quiet run ends with, as its test or script: see `recto-ffi`'s `qa::end_with_starter`.
    pub owner_pid: Option<u32>,
    pub open: Vec<PathBuf>,
}

/// Parses a launch's arguments: QA options only with `--qa`, what macOS adds passed over, and anything else that starts with `-` refused.
///
/// # Errors
///
/// What's wrong with them, in words, for the launch to end with.
pub fn parse(args: &[String]) -> Result<LaunchArgs, String> {
    let mut parsed = LaunchArgs::default();
    let mut words = args.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--qa" => parsed.qa = true,
            "--qa-memory-limit-mib" => {
                parsed.memory_limit_mib = Some(
                    words
                        .next()
                        .and_then(|value| value.parse().ok())
                        .ok_or("--qa-memory-limit-mib takes a number.")?,
                )
            }
            "--qa-time-limit-ms" => {
                parsed.time_limit = Some(Duration::from_millis(
                    words.next().and_then(|value| value.parse().ok()).ok_or("--qa-time-limit-ms takes a number.")?,
                ))
            }
            "--qa-owner-pid" => {
                parsed.owner_pid = Some(
                    words
                        .next()
                        .and_then(|value| value.parse().ok())
                        .filter(|pid| *pid > 1)
                        .ok_or("--qa-owner-pid takes a process number.")?,
                )
            }
            // What macOS adds to a launch from elsewhere, and a file to open.
            other if other.starts_with("-psn_") || other.starts_with("-NS") || other.starts_with("-Apple") => {
                let _ = words.next();
            }
            other if other.starts_with('-') => return Err(format!("`{}` isn't an option of this app.", cleaned(other))),
            other => parsed.open.push(PathBuf::from(other)),
        }
    }
    if !parsed.qa && (parsed.memory_limit_mib.is_some() || parsed.time_limit.is_some() || parsed.owner_pid.is_some()) {
        return Err("Those options are only for a QA run.".into());
    }
    Ok(parsed)
}

/// **Where a launch keeps its state:** the folders named, or the user's own, and for a quiet run only a new folder in a temp folder
/// ([`StatePaths::for_a_quiet_run`]): a quiet run's level starts at Full. What the bridge starts the app with.
///
/// # Errors
///
/// Why, in words: a folder that isn't allowed, or for a quiet run one that isn't throwaway.
pub fn state(args: &LaunchArgs) -> Result<StatePaths, String> {
    state_with(args, StatePaths::from_env().map_err(|error| error.message)?)
}

/// [`state`], with the folders named already chosen.
fn state_with(args: &LaunchArgs, named: StatePaths) -> Result<StatePaths, String> {
    if args.qa {
        named.for_a_quiet_run().map_err(|error| error.message)
    } else {
        Ok(named)
    }
}

/// A word of the arguments, made safe to say back: no control characters, and bounded.
fn cleaned(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(64).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: QA options work on a normal launch, so a shipped app can be started with its limits changed, or a file named at launch is taken for an option.
    #[test]
    fn qa_options_only_go_with_a_qa_run() {
        let words = |list: &[&str]| list.iter().map(|word| (*word).to_string()).collect::<Vec<_>>();
        assert!(parse(&words(&["--qa-memory-limit-mib", "64"])).is_err());
        assert!(parse(&words(&["--qa-time-limit-ms", "5"])).is_err());
        assert!(parse(&words(&["--qa-owner-pid", "5"])).is_err());
        assert_eq!(parse(&words(&["--qa", "--qa-owner-pid", "77"])).unwrap().owner_pid, Some(77));
        assert!(
            parse(&words(&["--qa", "--qa-owner-pid", "1"])).is_err(),
            "launchd adopts orphans, so it'd be no owner"
        );
        let parsed = parse(&words(&["--qa", "--qa-memory-limit-mib", "64", "/d/a.pdf"])).unwrap();
        assert!(parsed.qa);
        assert_eq!(parsed.memory_limit_mib, Some(64));
        assert_eq!(parsed.open, vec![PathBuf::from("/d/a.pdf")]);
        assert!(parse(&words(&["--nope"])).is_err());
        assert!(parse(&words(&["--qa-memory-limit-mib"])).is_err());
        assert_eq!(parse(&words(&["-psn_0_12345"])).unwrap(), LaunchArgs::default(), "what macOS adds");
    }

    /// Failure: the app starts a quiet run, whose level starts at Full, on state that isn't throwaway: with no state folder named (the user's own), or with
    /// one outside the temp folders, and says nothing of what to set. A launch that isn't quiet keeps whatever it's given.
    #[test]
    fn a_quiet_launch_takes_only_temp_state() {
        let quiet = LaunchArgs {
            qa: true,
            ..LaunchArgs::default()
        };
        let refused = state_with(&quiet, StatePaths::users_own().unwrap()).expect_err("none named");
        assert!(refused.contains(recto_control::state::STATE_ENV), "it says what to set: {refused}");
        if let Some(outside) = crate::testkit::headless::outside_the_tests_folders("state")
            && let Ok(named) = StatePaths::overridden(&outside)
        {
            assert!(state_with(&quiet, named).is_err(), "a folder outside the temp folders");
        }
        assert!(
            state_with(&LaunchArgs::default(), StatePaths::users_own().unwrap()).is_ok(),
            "a launch that isn't quiet"
        );
        let temp = recto_control::state::quiet_run_temp_folders()[0].join(format!("recto-launch-{}", std::process::id()));
        assert!(
            state_with(&quiet, StatePaths::overridden(&temp).unwrap()).is_ok(),
            "a new folder in the temp folder"
        );
    }
}
