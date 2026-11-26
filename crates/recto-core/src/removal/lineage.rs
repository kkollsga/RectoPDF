//! Which files are one document's history: a pure decision over a ledger of what the app has seen of the document
//! (`docs/design/removal.md` §3, the original backup row).
//!
//! A version is (volume UUID, inode, size, mtime_ns), a [`FileVersion`]. A block-level clone keeps the last three, so the volume's own UUID
//! (never `st_dev`, which belongs to the mount and is reused by the next disk: measured in `recto-platform`'s `volume_mac.rs`) keeps two
//! different disks apart, and a remounted disk is still the same one. Versions join only when both have the same real volume and a real
//! inode; one without joins nothing. Joined into one document: `At` links with the same version (a rename or a hard link), the two ends of
//! a `Saved`, and the current (version, path), which counts as an `At`. A path alone never joins; an `Unplaced` path is read on both
//! sides, so it is Unknown. A Save As destination is no link at all ([`Lineage::destinations`]). The target is everything joined to the current file; the rest is `Other`.
//!
//! **Fork rule.** A block copy of a disk has the original's volume UUID and so can't be told from it. If one version was saved into two
//! different versions the history has split in two, and the lineage says so (`Gap::Forked`): it is not complete. Nothing here opens a file.

use super::ledger::{FileVersion, Gap, Ledger};
use super::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// One thing the ledger saw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    /// The document, as this version, was at this path.
    At { version: FileVersion, path: PathBuf },
    /// A save of this app turned one version into the next.
    Saved { from: FileVersion, to: FileVersion },
    /// A file was here but its version couldn't be proven. The path is read on both sides, so whatever is named by it is Unknown.
    Unplaced { path: PathBuf },
}

/// Caps on a ledger, checked before anything is allocated for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LedgerLimits {
    pub max_links: u32,
    pub max_path_bytes: u32,
    pub max_total_path_bytes: u32,
}

/// Design limits, not measurements: 4,096 links, 4,096 bytes a path, 1 MiB of paths.
pub const LEDGER_LIMITS: LedgerLimits = LedgerLimits {
    max_links: 4_096,
    max_path_bytes: 4_096,
    max_total_path_bytes: 1 << 20,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Target,
    Other,
}

/// Which sides a name or version was seen on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sides {
    target: bool,
    other: bool,
}

impl Sides {
    pub fn with(self, side: Side) -> Self {
        match side {
            Side::Target => Self { target: true, ..self },
            Side::Other => Self { other: true, ..self },
        }
    }

    /// Everything either has seen.
    pub fn union(self, other: Sides) -> Self {
        Self {
            target: self.target || other.target,
            other: self.other || other.other,
        }
    }

    /// Target only is the target's (a Save's), or the source's history (a Save As keeps it); Other only is another document's. Both or neither
    /// can't be placed, so the plan refuses rather than guesses.
    pub fn owner(self, mode: TargetMode) -> Owner {
        match (self.target, self.other) {
            (true, false) if mode == TargetMode::Save => Owner::Target,
            (true, false) => Owner::SaveAsSource,
            (false, true) => Owner::Unrelated,
            _ => Owner::Unknown,
        }
    }
}

/// The ledger, split into the target's links and the others'.
#[derive(Debug)]
pub struct Lineage<'a> {
    links: &'a [Link],
    /// Where saves were going that no link names (never landed): not links, so not in [`Lineage::paths`].
    destinations: Vec<&'a Path>,
    path: &'a Path,
    current: FileVersion,
    target: Vec<bool>,
    gap: Option<Gap>,
    began_ns: i64,
}

fn path_bytes(path: &Path) -> usize {
    path.as_os_str().as_encoded_bytes().len()
}

/// Splits `ledger`'s links by whether each joins the document that is now `current` at `path`, or refuses with `TooLarge` over a cap.
pub fn lineage<'a>(ledger: &'a Ledger, current: FileVersion, path: &'a Path, limits: &LedgerLimits) -> Result<Lineage<'a>, Refusal> {
    let links = ledger.links.as_slice();
    if links.len() + ledger.destinations.len() > limits.max_links as usize {
        return Err(Refusal::TooLarge);
    }
    let mut total = path_bytes(path);
    let mut longest = total;
    for link in links {
        if let Link::At { path, .. } | Link::Unplaced { path } = link {
            let len = path_bytes(path);
            longest = longest.max(len);
            total = total.saturating_add(len);
        }
    }
    for path in &ledger.destinations {
        let len = path_bytes(path);
        longest = longest.max(len);
        total = total.saturating_add(len);
    }
    if longest > limits.max_path_bytes as usize || total > limits.max_total_path_bytes as usize {
        return Err(Refusal::TooLarge);
    }
    let mut joined = Joined::default();
    let current_version = current;
    let current = joined.node(current);
    // One node per link, from the first end: a `Saved` has joined its two ends by the time it is read back.
    let mut nodes = Vec::with_capacity(links.len());
    // Where each version was saved to, to see one saved into two.
    let mut saved_to: HashMap<usize, usize> = HashMap::new();
    let mut forks = Vec::new();
    for link in links {
        nodes.push(match link {
            Link::At { version, .. } => joined.node(*version),
            Link::Unplaced { .. } => joined.fresh(),
            Link::Saved { from, to } => {
                let (a, b) = (joined.node(*from), joined.node(*to));
                let first = *saved_to.entry(a).or_insert(b);
                if first != b {
                    forks.push(a);
                }
                joined.union(a, b);
                a
            }
        });
    }
    let root = joined.find(current);
    let forked = forks.into_iter().any(|n| joined.find(n) == root);
    let target = nodes.into_iter().map(|n| joined.find(n) == root).collect();
    // A destination a file was later seen at (`At`, or `Unplaced` as a file that was there) has landed, or was there already: it is that link's path, as before.
    let named: HashSet<&Path> = links
        .iter()
        .filter_map(|link| match link {
            Link::At { path, .. } | Link::Unplaced { path } => Some(path.as_path()),
            Link::Saved { .. } => None,
        })
        .collect();
    let destinations = ledger.destinations.iter().map(PathBuf::as_path).filter(|path| !named.contains(path)).collect();
    Ok(Lineage {
        links,
        destinations,
        path,
        current: current_version,
        target,
        gap: ledger.gap.or(forked.then_some(Gap::Forked)),
        began_ns: ledger.began_ns,
    })
}

/// Union-find over versions. A version that joins nothing gets a node of its own each time.
#[derive(Default)]
struct Joined {
    parent: Vec<usize>,
    by_version: HashMap<([u8; 16], u64, u64, i64), usize>,
}

impl Joined {
    fn fresh(&mut self) -> usize {
        let n = self.parent.len();
        self.parent.push(n);
        n
    }

    fn node(&mut self, version: FileVersion) -> usize {
        let Some(key) = version.join_key() else { return self.fresh() };
        if let Some(&n) = self.by_version.get(&key) {
            return n;
        }
        let n = self.fresh();
        self.by_version.insert(key, n);
        n
    }

    fn find(&mut self, mut n: usize) -> usize {
        while self.parent[n] != n {
            self.parent[n] = self.parent[self.parent[n]];
            n = self.parent[n];
        }
        n
    }

    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        self.parent[a] = b;
    }
}

fn side(target: bool) -> Side {
    if target { Side::Target } else { Side::Other }
}

impl Lineage<'_> {
    /// The current path (Target), then each `At` path, in input order; an `Unplaced` path on both sides.
    pub fn paths(&self) -> impl Iterator<Item = (&Path, Side)> {
        std::iter::once((self.path, Side::Target)).chain(self.links.iter().zip(&self.target).flat_map(|(link, &t)| {
            let (path, sides) = match link {
                Link::At { path, .. } => (Some(path.as_path()), [Some(side(t)), None]),
                Link::Unplaced { path } => (Some(path.as_path()), [Some(Side::Target), Some(Side::Other)]),
                Link::Saved { .. } => (None, [None, None]),
            };
            sides.into_iter().flatten().filter_map(move |s| path.map(|p| (p, s)))
        }))
    }

    /// The paths a save was going to that no file has been seen at since: a Save As that never landed (it failed, or crashed). Not links: no version, no side
    /// and no journal or backup is placed by one. Only the survey of the writer's temporaries reads them, to list their folders and read a temp named for one
    /// as unknown.
    pub fn destinations(&self) -> impl Iterator<Item = &Path> {
        self.destinations.iter().copied()
    }

    /// Each `At` version and both ends of each `Saved`, in input order.
    pub fn versions(&self) -> impl Iterator<Item = (FileVersion, Side)> {
        self.links.iter().zip(&self.target).flat_map(|(link, &t)| {
            let ends = match link {
                Link::At { version, .. } => [Some(*version), None],
                Link::Saved { from, to } => [Some(*from), Some(*to)],
                Link::Unplaced { .. } => [None, None],
            };
            ends.into_iter().flatten().map(move |v| (v, side(t)))
        })
    }

    /// The version the document is now: not among the links, since nothing has been saved from it yet.
    pub fn current(&self) -> FileVersion {
        self.current
    }

    /// Whether the ledger is the whole story of this document: nothing was lost, and the history did not fork.
    pub fn complete(&self) -> bool {
        self.gap.is_none()
    }

    /// The first reason `complete` is false.
    pub fn gap(&self) -> Option<Gap> {
        self.gap
    }

    /// When the ledger began: nothing older than this was recorded.
    pub fn began_ns(&self) -> i64 {
        self.began_ns
    }
}

#[cfg(test)]
mod tests;
