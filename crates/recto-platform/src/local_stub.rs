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
        Self {
            token,
            want_read,
            want_write,
            readable: false,
            writable: false,
            gone: false,
        }
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

pub fn file_system_name(_file: &File) -> io::Result<String> {
    unsupported()
}

pub fn open_private_file(_path: &Path) -> io::Result<File> {
    unsupported()
}

pub fn read_private_file(_path: &Path, _max_bytes: u64) -> io::Result<Option<Vec<u8>>> {
    unsupported()
}

pub fn write_private_file(_path: &Path, _bytes: &[u8]) -> io::Result<()> {
    unsupported()
}

pub fn write_private_file_unsynced(_path: &Path, _bytes: &[u8]) -> io::Result<()> {
    unsupported()
}

pub fn create_private_file(_path: &Path) -> io::Result<File> {
    unsupported()
}

pub fn exclude_from_backups(_path: &Path) -> io::Result<()> {
    unsupported()
}

pub fn is_excluded_from_backups(_path: &Path) -> bool {
    false
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
    pub disk_bytes_written: u64,
}

pub fn process_counters(_pid: u32) -> Option<ProcessCounters> {
    None
}

/// Replaces a file and syncs. There's no folder sync here yet: not used at run time on this platform.
pub fn write_private_file_durably(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_private_file(path, bytes)
}

pub fn open_private_for_writing(_path: &Path, _append: bool) -> io::Result<File> {
    unsupported()
}

pub fn make_private_file(_path: &Path) -> io::Result<()> {
    unsupported()
}

pub fn hand_folder_to_drive(_folder: &Path) -> io::Result<()> {
    Ok(())
}

pub fn clone_file(_from: &Path, _to: &Path) -> io::Result<()> {
    unsupported()
}

pub fn clone_source_in(file: &Path, _folder: &Path) -> io::Result<PathBuf> {
    Ok(file.to_path_buf())
}

pub fn hand_to_drive(file: &File) -> io::Result<()> {
    file.sync_all()
}

pub fn sync_folder_of(_path: &Path) -> io::Result<()> {
    Ok(())
}

pub fn can_be_replaced(_path: &Path) -> bool {
    false
}

pub fn make_folder_private(_path: &Path) -> io::Result<()> {
    unsupported()
}

pub fn swap_paths(_a: &Path, _b: &Path) -> io::Result<()> {
    unsupported()
}

pub fn processes_running(_executable: &Path) -> Vec<u32> {
    Vec::new()
}

pub fn process_path(_pid: u32) -> Option<PathBuf> {
    None
}

/// Can't be asked here, so every process counts as running, and nothing of another run's is ever removed.
pub fn process_exists(_pid: u32) -> bool {
    true
}

pub fn signal_process(_pid: u32, _kill: bool) -> bool {
    false
}

pub fn spawn_detached(_program: &Path, _args: &[String], _envs: &[(String, String)], _log: &Path) -> io::Result<u32> {
    unsupported()
}

/// Nothing on this platform can say where a file went.
pub fn path_of_file_number(_device: u64, _inode: u64) -> Option<PathBuf> {
    None
}

pub fn carry_attributes(_from: &Path, _to: &Path) -> io::Result<usize> {
    Ok(0)
}

pub fn carry_origin(_from: &Path, _to: &Path) -> io::Result<usize> {
    Ok(0)
}

pub fn attribute(_path: &Path, _name: &str) -> Option<Vec<u8>> {
    None
}

pub fn set_attribute(_path: &Path, _name: &str, _value: &[u8]) -> io::Result<()> {
    unsupported()
}
