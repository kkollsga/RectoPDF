//! What's on offer and when: pure, so all of it can be tested without a clock, a server or an app.

use crate::saved::Saved;
use serde_json::Value;

/// The most `latest.json` can be. Far more than it needs, and anything larger isn't one.
pub const MAX_LATEST_BYTES: usize = 64 * 1024;
const DAY_MS: u64 = 24 * 3600 * 1000;
/// One check a day.
pub const CHECK_EVERY_MS: u64 = DAY_MS;
/// A check due at launch waits this long, and none comes sooner after the last try.
pub const MIN_GAP_MS: u64 = 60 * 1000;
/// A failed check waits this long, doubling with each failure in a row, up to a day.
pub const RETRY_AFTER_MS: u64 = 3600 * 1000;
/// An automatic install waits until the user hasn't touched the app for this long. Owner, 2026-11-07: "let's update in a window the user hasn't touched the app for 30 minutes".
pub const IDLE_BEFORE_INSTALL_MS: u64 = 30 * 60 * 1000;
/// How soon an automatic install looks again when something's in flight. Only while one's waiting, so never at rest.
pub const BUSY_RETRY_MS: u64 = 60 * 1000;

/// A plain `MAJOR.MINOR.PATCH`. No `v`, no suffix, no fourth part, no leading zeros.
#[must_use]
pub fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let parts: Vec<&str> = text.split('.').collect();
    let [major, minor, patch] = parts.as_slice() else { return None };
    let number = |part: &str| -> Option<u32> {
        let plain = !part.is_empty() && part.len() <= 9 && part.bytes().all(|byte| byte.is_ascii_digit()) && (part == "0" || !part.starts_with('0'));
        plain.then(|| part.parse().ok()).flatten()
    };
    Some((number(major)?, number(minor)?, number(patch)?))
}

/// A system version: `15`, `15.4`, `15.4.1`.
#[must_use]
pub fn parse_system_version(text: &str) -> Option<(u32, u32, u32)> {
    let numbers = text.split('.').map(|part| if !part.is_empty() && part.len() <= 9 && part.bytes().all(|byte| byte.is_ascii_digit()) { part.parse().ok() } else { None }).collect::<Option<Vec<u32>>>()?;
    match numbers.as_slice() {
        [major] => Some((*major, 0, 0)),
        [major, minor] => Some((*major, *minor, 0)),
        [major, minor, patch] => Some((*major, *minor, *patch)),
        _ => None,
    }
}

/// `new` is a newer version than `current`, and both are plain.
#[must_use]
pub fn is_newer(new: &str, current: &str) -> bool {
    matches!((parse_version(new), parse_version(current)), (Some(new), Some(current)) if new > current)
}

/// The zip a release carries for this version.
#[must_use]
pub fn zip_name(version: &str) -> String {
    format!("RectoPDF-{version}.zip")
}

/// A release as its `latest.json` describes it, and only what passed. Nothing else from that file is kept or shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Latest {
    pub version: String,
    pub sha256: String,
    pub minimum_macos: (u32, u32, u32),
}

/// `latest.json` is hostile until this has said otherwise. `None` for anything but what a release writes: a plain version, its tag, its own zip, a well-formed checksum and a minimum macOS.
#[must_use]
pub fn parse_latest(data: &[u8]) -> Option<Latest> {
    if data.len() > MAX_LATEST_BYTES {
        return None;
    }
    let top: Value = serde_json::from_slice(data).ok()?;
    let version = top.get("version")?.as_str()?;
    parse_version(version)?;
    if top.get("tag")?.as_str()? != format!("v{version}") || top.get("zip")?.as_str()? != zip_name(version) {
        return None;
    }
    let sha256 = top.get("sha256")?.as_str()?;
    if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let minimum_macos = parse_system_version(top.get("minimum_macos")?.as_str()?)?;
    Some(Latest { version: version.to_owned(), sha256: sha256.to_ascii_lowercase(), minimum_macos })
}

impl Latest {
    /// Newer than what's running, and this Mac is new enough for it.
    #[must_use]
    pub fn on_offer(&self, current: &str, system: &str) -> bool {
        is_newer(&self.version, current) && parse_system_version(system).is_some_and(|system| system >= self.minimum_macos)
    }
}

/// After `failures` failed tries in a row: an hour, doubling, up to a day. With none, the least gap there is.
#[must_use]
pub fn retry_after(failures: u32) -> u64 {
    match failures {
        0 => MIN_GAP_MS,
        count => RETRY_AFTER_MS.saturating_mul(1 << (count - 1).min(5)).min(CHECK_EVERY_MS),
    }
}

/// When the next automatic check is due. Once a day, later after failures, never sooner than a minute after launch or the last try. A remembered time in the future, from a clock that's been moved, counts as now, so it can't put checks off for ever.
#[must_use]
pub fn next_check_at(saved: &Saved, launched_ms: u64, now_ms: u64) -> u64 {
    let last_check = saved.last_check_ms.map(|at| at.min(now_ms));
    let last_attempt = saved.last_attempt_ms.map(|at| at.min(now_ms));
    let due = if saved.failures > 0 {
        last_attempt.map_or(0, |at| at.saturating_add(retry_after(saved.failures)))
    } else {
        last_check.map_or(0, |at| at.saturating_add(CHECK_EVERY_MS))
    };
    due.max(launched_ms.saturating_add(MIN_GAP_MS)).max(last_attempt.map_or(0, |at| at.saturating_add(MIN_GAP_MS)))
}

/// What an automatic install does now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoStep {
    /// It's not going to: the switch is off, nothing's on offer, or this build doesn't.
    Never,
    /// Look again in this long.
    Wait(u64),
    Go,
}

/// What the 30-minute rule looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutoLook {
    pub build_may: bool,
    pub switch_on: bool,
    pub offered: bool,
    /// This version has already failed. It isn't tried by itself again: a newer one will be.
    pub version_failed: bool,
    pub since_input_ms: u64,
    pub busy: bool,
    /// How long until a failed install may be tried again.
    pub back_off_ms: u64,
}

/// The 30-minute rule. Whether or not the app is frontmost: it's how long since the user touched it.
#[must_use]
pub fn auto_step(look: AutoLook) -> AutoStep {
    if !(look.build_may && look.switch_on && look.offered) || look.version_failed {
        return AutoStep::Never;
    }
    if look.back_off_ms > 0 {
        return AutoStep::Wait(look.back_off_ms);
    }
    if look.since_input_ms < IDLE_BEFORE_INSTALL_MS {
        return AutoStep::Wait(IDLE_BEFORE_INSTALL_MS - look.since_input_ms);
    }
    if look.busy {
        return AutoStep::Wait(BUSY_RETRY_MS);
    }
    AutoStep::Go
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn good(version: &str) -> String {
        format!(r#"{{"version":"{version}","tag":"v{version}","minimum_macos":"15.0","zip":"RectoPDF-{version}.zip","sha256":"{SHA}","notes_url":"https://example.invalid"}}"#)
    }

    /// Failure: a version that isn't plain gets on offer, as a pre-release or a four-part build would, or one this reader can't order.
    #[test]
    fn only_a_plain_version_is_a_version() {
        assert_eq!(parse_version("0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse_version("10.20.30"), Some((10, 20, 30)));
        for bad in ["", "1", "1.2", "1.2.3.4", "v1.2.3", "1.2.3-beta", "1.2.x", "01.2.3", "1.02.3", " 1.2.3", "1.2.3 ", "1..3", "1.2.", "9999999999.0.0", "+1.2.3", "-1.2.3", "١.٢.٣"] {
            assert_eq!(parse_version(bad), None, "{bad}");
        }
        assert!(is_newer("0.1.10", "0.1.9"), "numbers, not text");
        assert!(!is_newer("0.1.9", "0.1.10"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
        assert!(!is_newer("0.2.0", "garbage"), "an app that can't say what it is takes nothing");
    }

    /// Failure: a garbled, tampered or hostile `latest.json` makes an offer, or brings down the app. Each of these is a field wrong in a way a bad release or a bad server could make it.
    #[test]
    fn a_hostile_latest_json_offers_nothing() {
        assert!(parse_latest(good("0.2.0").as_bytes()).is_some());
        let long = format!(r#"{{"version":"{}"}}"#, "9".repeat(MAX_LATEST_BYTES));
        let deep = "[".repeat(100_000);
        let cases: Vec<String> = vec![
            String::new(),
            "not json".into(),
            "[]".into(),
            "null".into(),
            deep,
            long,
            "\u{0}\u{1}\u{2}".into(),
            good("0.2.0").replace("\"version\":\"0.2.0\"", "\"version\":0.2"),
            good("0.2.0").replace("\"version\":\"0.2.0\"", "\"version\":\"0.2.0-beta\""),
            good("0.2.0").replace("\"tag\":\"v0.2.0\"", "\"tag\":\"v0.3.0\""),
            good("0.2.0").replace("\"tag\":\"v0.2.0\"", "\"tag\":\"0.2.0\""),
            good("0.2.0").replace("RectoPDF-0.2.0.zip", "../../evil.zip"),
            good("0.2.0").replace("RectoPDF-0.2.0.zip", "RectoPDF-0.1.0.zip"),
            good("0.2.0").replace("RectoPDF-0.2.0.zip", "https://evil.invalid/RectoPDF-0.2.0.zip"),
            good("0.2.0").replace(SHA, "aaaa"),
            good("0.2.0").replace(SHA, &"g".repeat(64)),
            good("0.2.0").replace(SHA, &"a".repeat(65)),
            good("0.2.0").replace("\"minimum_macos\":\"15.0\",", ""),
            good("0.2.0").replace("\"15.0\"", "\"soon\""),
            good("0.2.0").replace("\"15.0\"", "15"),
            good("0.2.0").replace("\"zip\":\"RectoPDF-0.2.0.zip\",", ""),
            good("0.2.0").replace(&format!(",\"sha256\":\"{SHA}\""), ""),
        ];
        for case in cases {
            assert_eq!(parse_latest(case.as_bytes()), None, "{}", case.chars().take(120).collect::<String>());
        }
        assert_eq!(parse_latest(&[0xFF, 0xFE, 0x80]), None, "not text");
    }

    /// Failure: a downgrade is offered, or the same version again, or one this Mac can't run. And what a release says about anything else stays out: only these three fields are kept.
    #[test]
    fn only_a_newer_release_this_mac_can_run_is_offered() {
        let latest = parse_latest(good("0.2.0").as_bytes()).unwrap();
        assert!(latest.on_offer("0.1.9", "15.0"));
        assert!(latest.on_offer("0.1.9", "27.0.1"));
        assert!(!latest.on_offer("0.2.0", "15.0"), "the same");
        assert!(!latest.on_offer("0.3.0", "15.0"), "a downgrade");
        assert!(!latest.on_offer("0.1.9", "14.7"), "too old a Mac");
        assert!(!latest.on_offer("0.1.9", "unknown"), "a Mac that can't say");
        assert_eq!(latest.minimum_macos, (15, 0, 0));
        assert_eq!(latest.sha256, SHA);
    }

    /// Failure: checks come more than once a day, or at launch, or straight after a failure, or a clock set back puts them off for ever. This is the schedule that's all the app's network use.
    #[test]
    fn the_schedule_is_once_a_day_and_backs_off() {
        let launched = 1_000_000_000_000;
        let fresh = Saved::default();
        assert_eq!(next_check_at(&fresh, launched, launched), launched + MIN_GAP_MS, "a minute after launch, at the soonest");
        let checked = Saved { last_check_ms: Some(launched), last_attempt_ms: Some(launched), ..Saved::default() };
        assert_eq!(next_check_at(&checked, launched, launched), launched + CHECK_EVERY_MS, "a day after the last");
        let old = Saved { last_check_ms: Some(launched - 3 * CHECK_EVERY_MS), last_attempt_ms: Some(launched - 3 * CHECK_EVERY_MS), ..Saved::default() };
        assert_eq!(next_check_at(&old, launched, launched), launched + MIN_GAP_MS, "overdue still waits for the minute");
        let failed = |failures| Saved { last_attempt_ms: Some(launched), last_check_ms: Some(launched - CHECK_EVERY_MS), failures, ..Saved::default() };
        assert_eq!(next_check_at(&failed(1), launched, launched), launched + 3600 * 1000);
        assert_eq!(next_check_at(&failed(2), launched, launched), launched + 2 * 3600 * 1000);
        assert_eq!(next_check_at(&failed(50), launched, launched), launched + CHECK_EVERY_MS, "up to a day");
        let future = Saved { last_check_ms: Some(u64::MAX), last_attempt_ms: Some(u64::MAX), ..Saved::default() };
        assert!(next_check_at(&future, launched, launched) <= launched + CHECK_EVERY_MS, "a clock moved back");
    }

    /// Failure: an automatic install goes while the user is using the app, or never goes, or goes with the switch off, in a development build, or over something in flight. The owner's 30-minute rule, frontmost or not: no sleeping, as the time is what's given.
    #[test]
    fn an_automatic_install_waits_for_thirty_quiet_minutes() {
        let look = AutoLook { build_may: true, switch_on: true, offered: true, version_failed: false, since_input_ms: IDLE_BEFORE_INSTALL_MS, busy: false, back_off_ms: 0 };
        assert_eq!(auto_step(look), AutoStep::Go, "thirty minutes exactly");
        assert_eq!(auto_step(AutoLook { since_input_ms: IDLE_BEFORE_INSTALL_MS - 1, ..look }), AutoStep::Wait(1), "a moment short");
        assert_eq!(auto_step(AutoLook { since_input_ms: 0, ..look }), AutoStep::Wait(IDLE_BEFORE_INSTALL_MS), "just touched");
        assert_eq!(auto_step(AutoLook { busy: true, ..look }), AutoStep::Wait(BUSY_RETRY_MS), "something in flight finishes first");
        assert_eq!(auto_step(AutoLook { switch_on: false, ..look }), AutoStep::Never);
        assert_eq!(auto_step(AutoLook { build_may: false, ..look }), AutoStep::Never, "a development build never replaces itself");
        assert_eq!(auto_step(AutoLook { offered: false, ..look }), AutoStep::Never);
        assert_eq!(auto_step(AutoLook { version_failed: true, ..look }), AutoStep::Never, "one that failed isn't tried again by itself");
        assert_eq!(auto_step(AutoLook { back_off_ms: 5, ..look }), AutoStep::Wait(5));
    }
}
