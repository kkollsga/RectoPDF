//! A test's way to make one job short of time, in a file of its own so `host.rs` doesn't grow for it. Only with the `test-seams` feature, which only this
//! crate's own tests turn on: a release build has no such function.

use crate::DocRenderer;
use std::time::Duration;

/// **For tests: runs `job` with `deadline` as the renderer's page deadline,** and puts the policy's own back after, however `job` ends. The policy is fixed when
/// a renderer is made, so a test of what a job past its deadline leaves behind could only make the whole renderer short of time, and the ordinary job after it
/// ran under that too, which a slow runner could fail on its own. Here only `job` is short: the job after it keeps the default. A free function, not a
/// method, so that [`DocRenderer`] has no method more in a shipping build for it (the ratchet counts methods per type). Only the host's own wait reads
/// the deadline this sets; a worker already started holds the deadline it was started with.
#[doc(hidden)]
pub fn under_page_deadline_for_test<T, R>(holder: &mut T, renderer: fn(&mut T) -> &mut DocRenderer, deadline: Duration, job: impl FnOnce(&mut T) -> R) -> R {
    let own = std::mem::replace(&mut renderer(holder).policy.page_deadline, deadline);
    let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(holder)));
    renderer(holder).policy.page_deadline = own;
    done.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}
