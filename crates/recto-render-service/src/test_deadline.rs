//! A test's way to make one job short of time, in a file of its own so `host.rs` doesn't grow for it. Only with the `test-seams` feature, which only this
//! crate's own tests turn on: a release build has no such function.

use crate::{DocRenderer, Policy};
use std::sync::OnceLock;
use std::time::Duration;

/// The page deadline every renderer made in this process starts with, once a test has set it.
static THIS_PROCESS: OnceLock<Duration> = OnceLock::new();

/// **For a test whose outcome would otherwise turn on the machine's speed:** every renderer made in this process from now on starts with `deadline`
/// as its page deadline (its host's wait and its worker's own), so a job that takes long on a starved machine (the hidden-text check's draw of a page at
/// 2x, which past the deadline is left unchecked) ends as it would on a quiet one. Set once a process; later calls change nothing. A test that wants a
/// deadline met sets it long (its own wait limit); a test of what a deadline does uses [`under_page_deadline_for_test`].
#[doc(hidden)]
pub fn page_deadline_for_this_process(deadline: Duration) {
    let _ = THIS_PROCESS.set(deadline);
}

pub(crate) fn this_process() -> Option<Duration> {
    THIS_PROCESS.get().copied()
}

/// **For tests: runs `job` with `deadline` as the renderer's page deadline,** and puts the policy's own back after, however `job` ends. The policy is fixed when
/// a renderer is made, so a test of what a job past its deadline leaves behind could only make the whole renderer short of time, and the ordinary job after it
/// ran under that too, which a slow runner could fail on its own. Here only `job` is short: the job after it keeps the default. A free function, not a
/// method, so that [`DocRenderer`] has no method more in a shipping build for it (the ratchet counts methods per type). Only the host's own wait reads
/// the deadline this sets; a worker already started holds the deadline it was started with.
#[doc(hidden)]
pub fn under_page_deadline_for_test<T, R>(holder: &mut T, renderer: fn(&mut T) -> &mut DocRenderer, deadline: Duration, job: impl FnOnce(&mut T) -> R) -> R {
    short_for(holder, renderer, |policy| &mut policy.page_deadline, deadline, job)
}

/// **For tests: the same for the start timeout,** so a worker that can't say hello in time (a Mac too busy to run it) is made without a hostile worker: only `job` is short, and the one after it starts a worker under the ordinary timeout.
#[doc(hidden)]
pub fn under_start_timeout_for_test<T, R>(holder: &mut T, renderer: fn(&mut T) -> &mut DocRenderer, timeout: Duration, job: impl FnOnce(&mut T) -> R) -> R {
    short_for(holder, renderer, |policy| &mut policy.start_timeout, timeout, job)
}

/// Runs `job` with the policy's `which` set to `short`, and puts the policy's own back after, however `job` ends.
fn short_for<T, R>(
    holder: &mut T,
    renderer: fn(&mut T) -> &mut DocRenderer,
    which: fn(&mut Policy) -> &mut Duration,
    short: Duration,
    job: impl FnOnce(&mut T) -> R,
) -> R {
    let own = std::mem::replace(which(&mut renderer(holder).policy), short);
    let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(holder)));
    *which(&mut renderer(holder).policy) = own;
    done.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}
