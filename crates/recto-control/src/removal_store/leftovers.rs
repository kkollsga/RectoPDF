//! **Clear leftovers** (`docs/design/removal.md` §10, D5): what makes a removal refuse, an unfinished save beside a document and the copies of the session
//! record kept for going back, moved to the Trash or deleted whole, each act proved by the identity pinned when it was looked at.
//!
//! - [`pin`]: what each leftover was (device, inode, kind, name, and its folder), recorded by the surveys as they list, through their caps.
//! - [`trash`]: the Trash an act moves a temp to, behind a trait, with the Mac's and a folder-backed fake for tests and quiet runs.
//! - [`act`]: the acts themselves, by descriptor, each stopped by what was pinned. Not wired to any command: the check, the leases, the words and the
//!   confirmation are the next branch's.
//!
//! Nothing here touches the live record (`RecordStore::dir()`), a document, a journal, a backup or the ledger.

pub mod act;
pub mod pin;
pub mod trash;

#[cfg(test)]
mod tests;
