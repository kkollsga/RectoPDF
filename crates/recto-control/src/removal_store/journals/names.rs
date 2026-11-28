//! What a name in a `journals/` folder says it is (`docs/design/removal.md` §3). Pure: names in, a kind out. Nothing is opened here.

use recto_session::record::{is_closed_journal_name, is_journal_name, is_kept_name};

/// What kind of thing a name is. Which store it belongs to, and how it is placed, is the adapter's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// `journal-K[-n].log` or `closed-K[-n].log`: placed by its first line.
    Journal,
    /// Any other `journal-*` or `closed-*` (`.unreadable`, `.kept`, `.not-merged`, `.earlier`, `.tmp<pid>`): placed by the same header rule.
    Variant,
    /// `pages-K[-n].ids`, or its temp `pages-K[-n].tmp<digits>`.
    PageIds,
    /// `sleep-K.bin`, or its temp `sleep-K.tmp<digits>`: placed by name.
    SleepFile,
    /// `kept-K[-n].keptpages`, or an older `.pdf`, or the temp a copy is written under (`kept-K[-n].tmp<digits>`): placed by name.
    KeptPages,
    /// What a crashed `lopdf::rewrite_atomically` leaves beside a kept copy: `.append-<16 hex>-<pid>-<n>.tmp` or `….d`. A partial PDF, never placed.
    KeptTemp,
    /// Anything else, a name that isn't text included. Unknown, and the plan refuses.
    Other,
}

/// `<key>[-<n>]` of a name that is `<prefix><key>[-<n>]<suffix>` or `<prefix><key>[-<n>].tmp<digits>`, when `journal-<that>.log` is a journal's name.
fn stem<'a>(name: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    let rest = name.strip_prefix(prefix)?;
    let stem = rest.strip_suffix(suffix).or_else(|| {
        let (stem, digits) = rest.rsplit_once(".tmp")?;
        (!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())).then_some(stem)
    })?;
    is_journal_name(&format!("journal-{stem}.log")).then_some(stem)
}

/// A name as the folder lists it. One that isn't text is `Other`, never skipped: this is the only place that decides it.
pub(super) fn kind_of(name: &std::ffi::OsStr) -> Kind {
    name.to_str().map_or(Kind::Other, kind)
}

pub(super) fn kind(name: &str) -> Kind {
    if is_journal_name(name) || is_closed_journal_name(name) {
        Kind::Journal
    } else if name.starts_with("journal-") || name.starts_with("closed-") {
        Kind::Variant
    } else if stem(name, "pages-", ".ids").is_some() {
        Kind::PageIds
    } else if stem(name, "sleep-", ".bin").is_some() {
        Kind::SleepFile
    } else if is_kept_name(name) || stem(name, "kept-", ".keptpages").is_some() {
        Kind::KeptPages
    } else if name.starts_with(".append-") && (name.ends_with(".tmp") || name.ends_with(".d")) {
        Kind::KeptTemp
    } else {
        Kind::Other
    }
}

/// The `<key>[-<n>]` of a sidecar's name, which names its journal as `journal-<that>.log`.
pub(super) fn sidecar_stem(name: &str) -> Option<&str> {
    stem(name, "pages-", ".ids")
}

/// The 16-digit key of a sleep or kept name: the version of the file it is about.
pub(super) fn version_key(name: &str) -> Option<&str> {
    let rest = match kind(name) {
        Kind::SleepFile => name.strip_prefix("sleep-")?,
        Kind::KeptPages => name.strip_prefix("kept-")?,
        _ => return None,
    };
    rest.get(..16).filter(|key| key.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a name a build writes is read as something else, so its file is skipped as "none found" or placed by the wrong rule.
    #[test]
    fn every_name_a_build_writes_has_its_kind() {
        let k = "0123456789abcdef";
        for (name, want) in [
            (format!("journal-{k}.log"), Kind::Journal),
            (format!("journal-{k}-3.log"), Kind::Journal),
            (format!("closed-{k}.log"), Kind::Journal),
            (format!("journal-{k}.log.unreadable"), Kind::Variant),
            (format!("closed-{k}.log.tmp123"), Kind::Variant),
            (format!("pages-{k}.ids"), Kind::PageIds),
            (format!("pages-{k}-2.tmp9"), Kind::PageIds),
            (format!("sleep-{k}.bin"), Kind::SleepFile),
            (format!("sleep-{k}.tmp77"), Kind::SleepFile),
            (format!("kept-{k}.keptpages"), Kind::KeptPages),
            (format!("kept-{k}-1.pdf"), Kind::KeptPages),
            (format!("kept-{k}.tmp4242"), Kind::KeptPages),
            (format!("kept-{k}-2.tmp4242"), Kind::KeptPages),
            (format!("kept-{k}.tmp"), Kind::Other),
            (format!(".append-{k}-12-0.tmp"), Kind::KeptTemp),
            (format!(".append-{k}-12-0.d"), Kind::KeptTemp),
            ("notes.txt".into(), Kind::Other),
            ("journal-zz.log".into(), Kind::Variant),
            (format!("pages-{k}.tmp"), Kind::Other),
            ("sleep-zz.bin".into(), Kind::Other),
        ] {
            assert_eq!(kind(&name), want, "{name}");
        }
        assert_eq!(kind_of(std::ffi::OsStr::new("journal-0123456789abcdef.log")), Kind::Journal);
        assert_eq!(version_key("kept-0123456789abcdef-1.pdf"), Some("0123456789abcdef"));
        assert_eq!(sidecar_stem("pages-0123456789abcdef-4.ids"), Some("0123456789abcdef-4"));
    }

    /// Failure: a name that isn't text (APFS can't make one, so this holds the pure decision; other file systems can) is read as a lossy text and
    /// takes the journal rule, or is skipped, so a file the survey can't name reads "none found". It is `Other`, which is Unknown.
    #[test]
    fn a_name_that_is_not_text_is_other() {
        use std::os::unix::ffi::OsStrExt as _;
        for bytes in [&b"journal-\xff.log"[..], b"pages-\xff.ids", b"kept-\xfe.pdf", b"\xff"] {
            assert_eq!(kind_of(std::ffi::OsStr::from_bytes(bytes)), Kind::Other, "{bytes:?}");
        }
    }
}
