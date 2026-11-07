//! Windows comes later. These keep everything above compiling, and fail plainly if anything tries to use them.

use crate::ProcessMemory;
use std::ffi::{CStr, OsStr, c_void};
use std::fs::File;
use std::io;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

pub const MAX_HANDLES: usize = 2;

fn unsupported<T>() -> io::Result<T> {
    Err(io::ErrorKind::Unsupported.into())
}

#[derive(Debug)]
pub struct Handle(());

impl Handle {
    pub fn from_file(_file: &File) -> io::Result<Self> {
        unsupported()
    }
    pub fn into_file(self) -> File {
        unreachable!("no handle can exist here")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wait {
    Ready,
    TimedOut,
}

#[derive(Debug)]
pub struct Channel(());

impl Channel {
    pub fn pair() -> io::Result<(Self, Self)> {
        unsupported()
    }
    pub fn inherited() -> io::Result<Self> {
        unsupported()
    }
    pub fn set_nonblocking(&self, _on: bool) -> io::Result<()> {
        unsupported()
    }
    pub fn send(&self, _bytes: &[u8], _handles: &[&Handle]) -> io::Result<usize> {
        unsupported()
    }
    pub fn recv(&self, _buf: &mut [u8], _handles: &mut Vec<Handle>) -> io::Result<usize> {
        unsupported()
    }
    pub fn wait_readable(&self, _timeout: Option<Duration>) -> io::Result<Wait> {
        unsupported()
    }
    pub fn wait_writable(&self, _timeout: Option<Duration>) -> io::Result<Wait> {
        unsupported()
    }
    pub fn peer_gone(&self) -> bool {
        true
    }
}

#[derive(Debug)]
pub struct SharedMemory(());

impl SharedMemory {
    pub fn create(_len: usize) -> io::Result<Self> {
        unsupported()
    }
    pub fn map(_handle: Handle, _len: usize) -> io::Result<Self> {
        unsupported()
    }
    pub fn len(&self) -> usize {
        0
    }
    pub fn is_empty(&self) -> bool {
        true
    }
    pub fn handle(&self) -> &Handle {
        unreachable!("no shared memory can exist here")
    }
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut []
    }
    pub fn copy_out(&self, _offset: usize, _out: &mut [u8]) -> bool {
        false
    }
    pub fn atomic_u64(&self, _offset: usize) -> Option<&AtomicU64> {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildExit {
    Code(i32),
    Signal(i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdleRule {
    pub after: Duration,
    pub memory_limit_bytes: u64,
}

#[derive(Debug)]
pub struct ChildProcess(());

impl ChildProcess {
    pub fn spawn(_exe: &Path, _args: &[&OsStr], _env: &[(&str, &str)], _channel: Channel, _rule: Option<IdleRule>) -> io::Result<Self> {
        unsupported()
    }
    pub fn pid(&self) -> u32 {
        0
    }
    pub fn memory(&self) -> Option<ProcessMemory> {
        None
    }
    pub fn try_wait(&self) -> Option<ChildExit> {
        None
    }
    pub fn kill_now(&self) -> bool {
        false
    }
    pub fn wait_bounded(&self, _limit: Duration) -> Option<ChildExit> {
        None
    }
    pub fn note_activity(&self, _busy: bool) {}
    pub fn on_end(&self, _action: Box<dyn FnOnce() + Send>) {}
}

pub fn process_memory(_pid: u32) -> Option<ProcessMemory> {
    None
}

pub(crate) fn memory_free_percent() -> Option<u32> {
    None
}

pub fn harden_this_process() {}

pub fn exit_now(code: i32) -> ! {
    std::process::exit(code)
}

pub fn library_file_name(stem: &str) -> String {
    format!("{stem}.dll")
}

#[derive(Debug)]
pub struct Library(());

impl Library {
    pub fn open(_path: &Path) -> io::Result<Self> {
        unsupported()
    }
    pub fn symbol(&self, _name: &CStr) -> Option<NonNull<c_void>> {
        None
    }
}
