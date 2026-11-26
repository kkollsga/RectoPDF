//! Folders held by descriptor come with the Mac. Off it nothing opens, so a survey that needs one reads as unlisted and refuses.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

pub use crate::folder_tree::{Identity, Kind, MAX_DEPTH, Pinned, Tree};

/// Never made here.
#[derive(Debug)]
pub struct Folder(());

/// What opening a folder found.
#[derive(Debug)]
pub enum Opened {
    Missing,
    Other,
    Folder(Folder),
}

/// One entry of a folder, as `fstatat` without following a link says.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: OsString,
    pub is_file: bool,
    pub links: u64,
    pub modified_ns: Option<i64>,
    pub dev: u64,
    pub ino: u64,
    pub kind: Kind,
}

/// Nothing is local off the Mac.
pub fn is_local(_path: &Path) -> bool {
    false
}

/// Nothing is looked at off the Mac.
pub fn identity_at(_path: &Path) -> Option<Identity> {
    None
}

impl Entry {
    pub fn identity(&self) -> Identity {
        Identity {
            dev: self.dev,
            ino: self.ino,
            kind: self.kind,
        }
    }
}

/// Never made here.
#[derive(Debug)]
pub struct Entries<'a>(std::marker::PhantomData<&'a Folder>);

impl Iterator for Entries<'_> {
    type Item = io::Result<Entry>;
    fn next(&mut self) -> Option<Self::Item> {
        None
    }
}

impl Folder {
    pub fn open(_path: &Path) -> Opened {
        Opened::Other
    }
    pub fn open_in(&self, _name: &OsStr) -> Opened {
        Opened::Other
    }
    pub fn entries(&self) -> io::Result<Entries<'_>> {
        Err(io::ErrorKind::Unsupported.into())
    }
    pub fn identity(&self) -> io::Result<Identity> {
        Err(io::ErrorKind::Unsupported.into())
    }
    pub fn look(&self, _name: &OsStr) -> io::Result<Entry> {
        Err(io::ErrorKind::Unsupported.into())
    }
    pub fn unlink_in(&self, _name: &OsStr, _folder: bool) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
    pub fn path(&self) -> io::Result<PathBuf> {
        Err(io::ErrorKind::Unsupported.into())
    }
    pub fn sync(&self) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
    pub fn open_private_in(&self, _name: &OsStr) -> io::Result<File> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

/// Taking a pinned folder away: nothing is, off the Mac.
pub mod remove {
    use super::{Folder, Kind, Tree};
    use std::ffi::{OsStr, OsString};
    use std::io;

    #[derive(Debug)]
    pub enum Failure {
        Changed(OsString),
        Unpinned(OsString),
        TooDeep,
        Stopped(OsString),
        NotDurable(OsString),
        Io(OsString, io::Error),
    }

    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    pub struct Removed {
        pub unlinked: usize,
        pub already_gone: usize,
    }

    pub struct Walk<'a> {
        pub rank: &'a dyn Fn(usize, &OsStr, Kind) -> u8,
        pub gate: &'a mut dyn FnMut(&OsStr) -> bool,
    }

    pub fn remove_tree_in(_parent: &Folder, name: &OsStr, _tree: &Tree, _walk: &mut Walk<'_>) -> Result<Removed, Failure> {
        Err(Failure::Io(name.to_owned(), io::ErrorKind::Unsupported.into()))
    }
}
