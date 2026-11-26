//! Folders held by descriptor come with the Mac. Off it nothing opens, so a survey that needs one reads as unlisted and refuses.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::path::Path;

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
    pub fn open_file(&self, _name: &OsStr) -> io::Result<File> {
        Err(io::ErrorKind::Unsupported.into())
    }
}
