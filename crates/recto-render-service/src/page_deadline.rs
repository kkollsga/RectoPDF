//! **The page deadline a renderer's policy starts with:** a job running longer has its worker killed (`Policy::page_deadline`). In a file of its own so
//! `host.rs` doesn't grow for the one thing a test may set here.

use std::time::Duration;

/// Platform §2.6. The slowest real page at 2x took 939 ms.
const PAGE_DEADLINE: Duration = Duration::from_secs(5);

/// What a new policy starts with: [`PAGE_DEADLINE`], or with the `test-seams` feature what a test set for its whole process
/// (`page_deadline_for_this_process`).
pub(crate) fn starting() -> Duration {
    #[cfg(feature = "test-seams")]
    if let Some(set) = crate::test_deadline::this_process() {
        return set;
    }
    PAGE_DEADLINE
}
