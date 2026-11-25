//! The canonical survey: what the per-store adapters found, turned into the one `Inventory` a plan is made from
//! (`docs/design/removal.md` §4).
//!
//! **Order contract.**
//! 1. Outside `recto-core`, an `Inventory` comes only from [`Surveyed::inventory`].
//! 2. [`Survey::finish`] sorts by (owner rank, `digest::store_code`, key bytes). The rank is an explicit `match`: Target 0, SaveAsSource 1,
//!    Unrelated 2, Shared 3, Unknown 4. Ids are position + 1.
//! 3. So the same set of artifacts in any order gives the same ids, steps and digest. Other documents' artifacts never renumber the target's
//!    (its ids are 1..=T); they change only the report's counts.
//! 4. A key names an artifact within its store. A later adapter finds the file again by listing its own folder and rechecks it before any
//!    change. Core holds no names or paths.
//! 5. A store is complete only if its adapter called [`Survey::mark_complete`] after listing all of it. The default is incomplete.
//!
//! Bounded: the survey holds at most `Limits::max_artifacts` entries, and refuses, staying refused, at the next. Nothing here opens a file.

use super::digest::store_code;
use super::*;
use sha2::{Digest, Sha256};

/// A fixed-size name for one artifact within its store: a hash of the store's code and the root-relative name, so core never holds a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ArtifactKey([u8; 32]);

impl ArtifactKey {
    /// `SHA-256("recto-removal-artifact/1\0" ‖ store code u8 ‖ name length u32 LE ‖ name)`. `name` is root-relative, never a path.
    pub fn of(store: Store, name: &[u8]) -> Self {
        let mut h = Sha256::new();
        h.update(b"recto-removal-artifact/1\0");
        h.update([store_code(store)]);
        h.update((name.len() as u32).to_le_bytes());
        h.update(name);
        Self(h.finalize().into())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// One artifact an adapter found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Found {
    pub store: Store,
    pub key: ArtifactKey,
    pub owner: Owner,
    pub format: Format,
}

/// What the adapters add to, one store at a time.
#[derive(Debug)]
pub struct Survey {
    max: u32,
    complete: StoreSet,
    found: Vec<Found>,
    over: bool,
}

impl Survey {
    /// Allocates nothing.
    pub fn new(limits: &Limits) -> Self {
        Self {
            max: limits.max_artifacts,
            complete: StoreSet::empty(),
            found: Vec::new(),
            over: false,
        }
    }

    /// The `max + 1`th artifact is `TooLarge`, and the survey stays refused: a cut survey must not read as "none found".
    pub fn add(&mut self, found: Found) -> Result<(), Refusal> {
        if self.over || self.found.len() >= self.max as usize {
            self.over = true;
            return Err(Refusal::TooLarge);
        }
        self.found.push(found);
        Ok(())
    }

    /// The adapter listed all of `store`.
    pub fn mark_complete(&mut self, store: Store) {
        self.complete = self.complete.with(store);
    }

    /// Sorts into the order contract. `TooLarge` if the cap was passed, `Inconsistent` if one (store, key) was found twice.
    pub fn finish(mut self) -> Result<Surveyed, Refusal> {
        if self.over {
            return Err(Refusal::TooLarge);
        }
        self.found.sort_unstable_by_key(|f| (store_code(f.store), f.key));
        if self.found.windows(2).any(|w| w[0].store == w[1].store && w[0].key == w[1].key) {
            return Err(Refusal::Inconsistent);
        }
        self.found.sort_unstable_by_key(|f| (owner_rank(f.owner), store_code(f.store), f.key));
        let artifacts = self
            .found
            .iter()
            .enumerate()
            .map(|(i, f)| Artifact {
                id: ArtifactId(i as u32 + 1),
                store: f.store,
                owner: f.owner,
                format: f.format,
            })
            .collect();
        let keys = self.found.iter().map(|f| f.key).collect();
        Ok(Surveyed {
            complete: self.complete,
            artifacts,
            keys,
        })
    }
}

/// An explicit table, not a derived `Ord`: a reordered variant must not renumber every artifact.
fn owner_rank(owner: Owner) -> u8 {
    match owner {
        Owner::Target => 0,
        Owner::SaveAsSource => 1,
        Owner::Unrelated => 2,
        Owner::Shared => 3,
        Owner::Unknown => 4,
    }
}

/// A finished survey: the inventory a plan is made from, and each id's key for the adapter that acts on it.
#[derive(Debug)]
pub struct Surveyed {
    complete: StoreSet,
    artifacts: Vec<Artifact>,
    keys: Vec<ArtifactKey>,
}

impl Surveyed {
    pub fn inventory(&self) -> Inventory<'_> {
        Inventory::new(self.complete, &self.artifacts)
    }

    /// The store and key an id was given for; `None` for 0 or past the end.
    pub fn key(&self, id: ArtifactId) -> Option<(Store, ArtifactKey)> {
        let at = (id.0 as usize).checked_sub(1)?;
        Some((self.artifacts.get(at)?.store, *self.keys.get(at)?))
    }
}

#[cfg(test)]
mod tests;
