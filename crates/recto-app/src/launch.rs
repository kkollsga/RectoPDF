//! **What a launch of the app was asked for:** the one parser of its arguments, so whether a launch is quiet, decided before the toolkit does anything, and what the rest
//! of the launch does can't disagree.

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
}
