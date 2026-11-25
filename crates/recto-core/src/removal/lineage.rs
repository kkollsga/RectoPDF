//! Which files are one document's history: a pure decision over a ledger of what the app has seen of the document
//! (`docs/design/removal.md` §3, the original backup row).
//!
//! A version is (device, inode, size, mtime_ns): a block-level clone or a restored image keeps the last three, so without the device two independent copies would join (`docs/design/removal.md` §4). The device separates only volumes mounted under different numbers: `st_dev` belongs to a mount, an ejected image and an attached clone can reuse one number and then join, and that reuse must be settled before any purge is wired (D2b/D3). A volume that comes back under another number splits the document: its old path lands on both sides (Unknown, the plan refuses) or only on Other (kept), both safe. Inode 0 joins nothing by version. Joined into one document: `At` links
//! with the same version (a rename or a hard link), the two ends of a `Saved`, and the current (stamp, path), which counts as an `At`. A path
//! alone never joins. The target is everything joined to the current file; the rest is `Other`. Nothing here opens a file.

use super::*;
use recto_platform::FileStamp;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One thing the ledger saw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    /// The document, as this version, was at this path.
    At { stamp: FileStamp, path: PathBuf },
    /// A save of this app turned one version into the next.
    Saved { from: FileStamp, to: FileStamp },
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
    path: &'a Path,
    target: Vec<bool>,
}

fn path_bytes(path: &Path) -> usize {
    path.as_os_str().as_encoded_bytes().len()
}

/// Splits `links` by whether each joins the document that is now `stamp` at `path`, or refuses with `TooLarge` over a cap.
pub fn lineage<'a>(links: &'a [Link], stamp: FileStamp, path: &'a Path, limits: &LedgerLimits) -> Result<Lineage<'a>, Refusal> {
    if links.len() > limits.max_links as usize {
        return Err(Refusal::TooLarge);
    }
    let mut total = path_bytes(path);
    let mut longest = total;
    for link in links {
        if let Link::At { path, .. } = link {
            let len = path_bytes(path);
            longest = longest.max(len);
            total = total.saturating_add(len);
        }
    }
    if longest > limits.max_path_bytes as usize || total > limits.max_total_path_bytes as usize {
        return Err(Refusal::TooLarge);
    }
    let mut joined = Joined::default();
    let current = joined.node(stamp);
    // One node per link, from the first end: a `Saved` has joined its two ends by the time it is read back.
    let mut nodes = Vec::with_capacity(links.len());
    for link in links {
        nodes.push(match link {
            Link::At { stamp, .. } => joined.node(*stamp),
            Link::Saved { from, to } => {
                let (a, b) = (joined.node(*from), joined.node(*to));
                joined.union(a, b);
                a
            }
        });
    }
    let root = joined.find(current);
    let target = nodes.into_iter().map(|n| joined.find(n) == root).collect();
    Ok(Lineage { links, path, target })
}

/// Union-find over versions. A version with inode 0 gets a node of its own each time, so it joins nothing.
#[derive(Default)]
struct Joined {
    parent: Vec<usize>,
    by_version: HashMap<(u64, u64, u64, i64), usize>,
}

impl Joined {
    fn node(&mut self, stamp: FileStamp) -> usize {
        let fresh = self.parent.len();
        if stamp.inode == 0 {
            self.parent.push(fresh);
            return fresh;
        }
        let node = *self.by_version.entry((stamp.device, stamp.inode, stamp.size, stamp.mtime_ns)).or_insert(fresh);
        if node == fresh {
            self.parent.push(fresh);
        }
        node
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
    /// The current path (Target), then each `At` path, in input order.
    pub fn paths(&self) -> impl Iterator<Item = (&Path, Side)> {
        std::iter::once((self.path, Side::Target)).chain(self.links.iter().zip(&self.target).filter_map(|(link, &t)| match link {
            Link::At { path, .. } => Some((path.as_path(), side(t))),
            Link::Saved { .. } => None,
        }))
    }

    /// Each `At` stamp and both ends of each `Saved`, in input order.
    pub fn stamps(&self) -> impl Iterator<Item = (FileStamp, Side)> {
        self.links.iter().zip(&self.target).flat_map(|(link, &t)| {
            let ends = match link {
                Link::At { stamp, .. } => [Some(*stamp), None],
                Link::Saved { from, to } => [Some(*from), Some(*to)],
            };
            ends.into_iter().flatten().map(move |s| (s, side(t)))
        })
    }
}

#[cfg(test)]
mod tests;
