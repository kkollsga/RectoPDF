//! What a folder's entries were when they were looked at, so a later act can prove it is acting on the same things (`docs/design/removal.md` §10, D5).
//! Plain data with no system calls, shared by the Mac's and the stand-in's `folder`.

use std::ffi::OsString;

/// What an entry is, as `fstatat` without following a link says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Folder,
    Link,
    /// A pipe, a socket, a device.
    Other,
}

/// What names one thing on one volume: the device and inode number, and what kind it is. A name that now leads to another identity is another thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub dev: u64,
    pub ino: u64,
    pub kind: Kind,
}

/// The most folders deep a pinned copy goes below its own folder (a session copy holds `journals/` and nothing deeper; three is a design limit).
pub const MAX_DEPTH: usize = 3;

/// One entry of a pinned folder: its name and what it was, and, for a folder, what was in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pinned {
    pub name: OsString,
    pub id: Identity,
    pub below: Option<Tree>,
}

/// A folder as it was pinned: itself, and each entry it held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    pub id: Identity,
    pub entries: Vec<Pinned>,
}
