//! **Whose a test's temp folder is.** An update test's bed lives in `recto-update-e2e-<pid>-<wall ms>-tree<hash>.noindex` in temp, and the system registers the copy of the app inside it. Worktrees share temp, so the name says which process made it, when, and from which checkout: the `registrations` stage blames a tree only for what its own tests left, and never changes what another tree's tests still own (2026-11-13: one tree's stage blamed, and forgot, a copy another tree's update test had made 3 s before).
//!
//! Shared by the beds (`restore/update.rs`) and the stage (`registrations.rs`), which `#[path]` it in; plain logic, tested in process.

use std::path::Path;

/// What the folder's name says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Made {
    pub pid: u32,
    pub at_ms: u64,
    /// The checkout's hash, or none in a name from before the hash was there.
    pub tree: Option<String>,
}

/// The checkout's short hash (FNV-1a of its path): the same in every test binary of the checkout, different between worktrees.
pub fn tree_of(checkout: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in checkout.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:08x}", hash >> 32)
}

/// The folder's name for a bed made by this process, now, from this checkout. `.noindex`, so Spotlight leaves it alone.
pub fn name(pid: u32, at_ms: u64, checkout: &Path) -> String {
    format!("recto-update-e2e-{pid}-{at_ms}-tree{}.noindex", tree_of(checkout))
}

/// Reads a name back: none for anything that isn't a bed's.
pub fn parse(name: &str) -> Option<Made> {
    let rest = name.strip_prefix("recto-update-e2e-")?;
    let rest = rest.strip_suffix(".noindex").unwrap_or(rest);
    let mut parts = rest.split('-');
    let pid = parts.next()?.parse().ok()?;
    let at_ms = parts.next()?.parse().ok()?;
    let tree = parts.next().and_then(|part| part.strip_prefix("tree")).map(str::to_owned);
    Some(Made { pid, at_ms, tree })
}
