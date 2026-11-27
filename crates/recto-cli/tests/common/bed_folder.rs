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

/// The folder name of a test's own copy of the app (`OwnCopy`), beside the build: the test binary's process and a number.
pub fn own_name(pid: u32, number: u32) -> String {
    format!("own-{pid}-{number}")
}

/// The process that made an `own-*` folder, or none for a name that isn't one.
pub fn parse_own(name: &str) -> Option<u32> {
    let mut parts = name.strip_prefix("own-")?.split('-');
    let pid = parts.next()?.parse().ok()?;
    parts.next()?.parse::<u32>().ok()?;
    parts.next().is_none().then_some(pid)
}

/// Removes a bed's folder and does not stop at the first refusal. A process that is still writing into the folder (the installer after its swap, a new app's first files) makes one `remove_dir_all` leave a file or a folder behind (`ENOTEMPTY`), and a test that ignores that leaves a folder for the `registrations` stage to find (2026-11-27, twice). Tries again, in steps of `step`, until the folder is gone or `within` is up. Whether it is gone, and what the last refusal was if it isn't.
pub fn remove_when_quiet(
    folder: &Path,
    within: std::time::Duration,
    step: std::time::Duration,
    mut remove: impl FnMut(&Path) -> std::io::Result<()>,
) -> Result<(), String> {
    let began = std::time::Instant::now();
    loop {
        let last = match remove(folder) {
            Ok(()) => None,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => Some(error),
        };
        // Gone is what the disk says, not what the call said: a writer may have made it again.
        if !folder.exists() {
            return Ok(());
        }
        if began.elapsed() >= within {
            return Err(format!(
                "{} was still there {:.1} s after its test ended ({})",
                folder.display(),
                began.elapsed().as_secs_f64(),
                last.map_or("removed, then made again by something still running".to_owned(), |error| error.to_string())
            ));
        }
        std::thread::sleep(step); // ratchet: poll
    }
}
