//! Windows comes later. This keeps everything above compiling, and fails plainly if anything tries to use it.

use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::path::Path;
use std::process::ExitStatus;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    Utility,
    Inherited,
}

#[derive(Debug)]
pub struct Spawned {
    pub child: Helper,
    pub stdin: File,
    pub stdout: File,
}

#[derive(Debug)]
pub struct Helper(());

pub const DOCUMENT_FD: i32 = 3;

pub const DOCUMENT_PATH: &str = "/dev/fd/3";

pub fn spawn(_exe: &Path, _env: &[(&str, &OsStr)], _priority: Priority) -> io::Result<Spawned> {
    Err(io::ErrorKind::Unsupported.into())
}

pub fn spawn_with_document(_exe: &Path, _env: &[(&str, &OsStr)], _priority: Priority, _document: &File) -> io::Result<Spawned> {
    Err(io::ErrorKind::Unsupported.into())
}

pub fn inherited_document() -> io::Result<File> {
    Err(io::ErrorKind::Unsupported.into())
}

impl Helper {
    pub fn id(&self) -> u32 {
        0
    }
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        Err(io::ErrorKind::Unsupported.into())
    }
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        Err(io::ErrorKind::Unsupported.into())
    }
    pub fn kill(&mut self) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
}
