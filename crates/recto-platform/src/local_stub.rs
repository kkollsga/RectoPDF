//! Windows comes later, with a named pipe and an owner-only DACL. These keep everything above compiling, and fail plainly if anything tries to use them.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MAX_SOCKET_PATH: usize = 103;

fn unsupported<T>() -> io::Result<T> {
    Err(io::ErrorKind::Unsupported.into())
}

pub fn current_uid() -> u32 {
    0
}

pub fn real_home() -> Option<PathBuf> {
    None
}

pub fn system_temp_dir() -> Option<PathBuf> {
    None
}

#[derive(Debug)]
pub struct Listener(());

impl Listener {
    pub fn bind(_path: &Path) -> io::Result<Self> {
        unsupported()
    }
    pub fn accept(&self) -> io::Result<Option<Stream>> {
        unsupported()
    }
    pub fn token(&self) -> Token {
        Token(())
    }
}

#[derive(Debug)]
pub struct Stream(());

impl Stream {
    pub fn connect(_path: &Path) -> io::Result<Self> {
        unsupported()
    }
    pub fn pair() -> io::Result<(Self, Self)> {
        unsupported()
    }
    pub fn peer_uid(&self) -> io::Result<u32> {
        unsupported()
    }
    pub fn set_nonblocking(&self, _on: bool) -> io::Result<()> {
        unsupported()
    }
    pub fn set_read_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        unsupported()
    }
    pub fn set_write_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        unsupported()
    }
    pub fn try_clone(&self) -> io::Result<Self> {
        unsupported()
    }
    pub fn token(&self) -> Token {
        Token(())
    }
}

impl Read for &Stream {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        unsupported()
    }
}

impl Write for &Stream {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        unsupported()
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token(());

impl Token {
    pub const NONE: Self = Self(());
}

#[derive(Clone, Copy, Debug)]
pub struct PollEntry {
    pub token: Token,
    pub want_read: bool,
    pub want_write: bool,
    pub readable: bool,
    pub writable: bool,
    pub gone: bool,
}

impl PollEntry {
    pub fn new(token: Token, want_read: bool, want_write: bool) -> Self {
        Self { token, want_read, want_write, readable: false, writable: false, gone: false }
    }
}

pub fn poll(_entries: &mut [PollEntry], _timeout: Option<Duration>) -> io::Result<()> {
    unsupported()
}

pub fn ensure_private_dir(_path: &Path) -> io::Result<()> {
    unsupported()
}

pub fn check_private_dir(_path: &Path) -> io::Result<()> {
    unsupported()
}

pub fn read_private_file(_path: &Path, _max_bytes: u64) -> io::Result<Option<Vec<u8>>> {
    unsupported()
}

pub fn write_private_file(_path: &Path, _bytes: &[u8]) -> io::Result<()> {
    unsupported()
}

pub fn append_private_file(_path: &Path) -> io::Result<File> {
    unsupported()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocketFile {
    Nothing,
    OwnSocket,
    Other,
}

pub fn socket_file(_path: &Path) -> io::Result<SocketFile> {
    unsupported()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcessCounters {
    pub cpu_ns: u64,
    pub wakeups: u64,
    pub context_switches: u64,
    pub threads: u32,
}

pub fn process_counters(_pid: u32) -> Option<ProcessCounters> {
    None
}
