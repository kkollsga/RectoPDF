//! Failure diagnostics must not keep a successful quiet run alive or block past their own deadline.

pub(super) fn assert_rest(settling: &[(u64, u64)], resting: &[(u64, u64)], evidence: impl FnOnce() -> String) {
    let why = if !settling.last().is_some_and(|window| window.0 <= 8) {
        Some("it never came to rest")
    } else if !resting.iter().all(|window| window.0 <= 80) {
        Some("it wakes at rest")
    } else {
        None
    };
    if let Some(why) = why {
        let evidence = std::panic::catch_unwind(std::panic::AssertUnwindSafe(evidence)).unwrap_or_else(|panic| {
            let cause = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("unknown diagnostic failure");
            format!("evidence collection failed: {cause}")
        });
        panic!("{why}: {evidence}");
    }
}

/// A bounded excerpt for a failed assertion; the complete bounded report belongs in its evidence folder.
pub(super) fn stack_summary(text: &str) -> String {
    let from = if text.starts_with("sample failed") {
        0
    } else {
        text.find("Call graph:").unwrap_or(0)
    };
    let to = text[from..].find("Binary Images:").map_or(text.len(), |offset| from + offset);
    text[from..to]
        .lines()
        .take(320)
        .map(|line| format!("{}\n", line.chars().take(130).collect::<String>()))
        .collect()
}
