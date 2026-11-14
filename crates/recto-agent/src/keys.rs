//! **The person's passwords, lent to their own commands** (PLAN, "the person can't print a document they opened with its password", design 2026-11-14):
//! what the person typed to open a document, so their own Find and Print read it as the page view draws it. An agent never gets them, by two locks:
//!
//! - **The door.** A host builds an access holding these only on the person's own route, which nothing the server holds can reach
//!   (`recto_control::host::SessionHost::run_for_person`); what the server calls (`Host::run`, `Host::check`) has none, whatever actor it's given.
//! - **The actor.** Asked for them, an access gives them only to an actor who may see a protected document ([`crate::Actor::sees_protected`]): the person.
//!
//! A password is looked up by **the open file's own identity** (device and inode, `recto_platform::secret::open_file_identity`), never by what's at a path
//! now, so a file put at the path after the person typed the password is never given it.

use recto_core::Password;
use std::sync::Arc;

/// The keyring's look-up by a file's device and inode.
type ByIdentity = dyn Fn((u64, u64)) -> Option<Arc<Password>> + Send + Sync;

/// **The person's passwords:** a look-up into the app's keyring, by which file each was typed for. Cloned cheaply; holds no password itself, only the way
/// to ask for one, so a password lives as long as the keyring and its own holders keep it.
#[derive(Clone)]
pub struct PersonsKeys(Arc<ByIdentity>);

impl std::fmt::Debug for PersonsKeys {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PersonsKeys")
    }
}

impl PersonsKeys {
    /// The keys looked up by `by_identity`, a file's device and inode: the app's keyring's `get`.
    pub fn new(by_identity: impl Fn((u64, u64)) -> Option<Arc<Password>> + Send + Sync + 'static) -> Self {
        Self(Arc::new(by_identity))
    }

    /// **The password typed for this open file,** by the file's own identity: not for whatever is at its path now. None for a file nobody typed one for,
    /// or one whose identity can't be read.
    pub fn for_file(&self, file: &std::fs::File) -> Option<Arc<Password>> {
        (self.0)(recto_platform::secret::open_file_identity(file)?)
    }

    /// **Find's look-up, by path,** as the person's Find has always looked a file up ([`crate::find::FindPlan::with_keys`]): the identity of the file at
    /// the path when Find reads it.
    pub fn for_find(&self) -> crate::find::KeyLookup {
        let keys = self.clone();
        Arc::new(move |path: &std::path::Path| (keys.0)(recto_platform::secret::file_identity(path)?))
    }
}
