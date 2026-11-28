//! **A use of the Find key, counted.** Everything that opens a corpus (the open writes `app/used`) or writes an agent's folder does it while it holds the key, so an indexer that counts the uses it has handed out can tell a removal when none is left (`docs/design/removal.md` §10, "Step 4 (quiescence) design"). [`Indexing::find_key`](super::Indexing::find_key) returns one of these: it derefs to the key, and its being dropped is the end of the use.
//!
//! No thread, timer or buffer: a reference to the key and, when counted, one to whoever counts. A plain one (a host that keeps no count, or a test) counts nothing.

use recto_platform::find_key::FindKey;
use std::ops::Deref;

/// Whoever counts uses: told once for each use that ends.
pub trait UseEnded: Sync {
    fn use_ended(&self);
}

/// A use of the caches for something that only deletes, with no key. One place a use ends: [`KeyUse`] holds one. Not `Clone`.
#[must_use = "a use of the caches ends when this is dropped: hold it until the last delete"]
pub struct CacheUse<'a> {
    counted: Option<&'a dyn UseEnded>,
}

impl CacheUse<'static> {
    /// A use nobody counts.
    pub fn plain() -> Self {
        Self { counted: None }
    }
}

impl<'a> CacheUse<'a> {
    /// A use `counter` has already counted, and is told of when it ends.
    pub fn counted(counter: &'a dyn UseEnded) -> Self {
        Self { counted: Some(counter) }
    }
}

impl Drop for CacheUse<'_> {
    fn drop(&mut self) {
        if let Some(counter) = self.counted {
            counter.use_ended();
        }
    }
}

impl std::fmt::Debug for CacheUse<'_> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("CacheUse").field("counted", &self.counted.is_some()).finish()
    }
}

/// The key, for as long as one thing uses it. Not `Clone`: a second use is a second `find_key`. Hold it for as long as what it's used for writes.
#[must_use = "a use of the key ends when this is dropped: hold it for as long as what's opened with the key can write"]
pub struct KeyUse<'a> {
    key: &'a FindKey,
    // Dropped with the key's use: the one place a use ends.
    use_of_caches: CacheUse<'a>,
}

impl<'a> KeyUse<'a> {
    /// A use nobody counts.
    pub fn plain(key: &'a FindKey) -> Self {
        Self {
            key,
            use_of_caches: CacheUse { counted: None },
        }
    }

    /// A use `counter` has already counted, and is told of when it ends.
    pub fn counted(key: &'a FindKey, counter: &'a dyn UseEnded) -> Self {
        Self {
            key,
            use_of_caches: CacheUse::counted(counter),
        }
    }
}

impl Deref for KeyUse<'_> {
    type Target = FindKey;

    fn deref(&self) -> &FindKey {
        self.key
    }
}

impl std::fmt::Debug for KeyUse<'_> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the key.
        out.debug_struct("KeyUse")
            .field("counted", &self.use_of_caches.counted.is_some())
            .finish_non_exhaustive()
    }
}
