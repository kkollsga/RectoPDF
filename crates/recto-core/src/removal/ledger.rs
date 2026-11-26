//! The provenance ledger's format: a pure encoder and a bounded reader (`docs/design/removal.md` §10, D2b). Nothing here opens a file.
//!
//! The file is a 36-byte header and then frames, appended and never rewritten. All integers are little-endian.
//! - Header: `RECTOLEDGER\0` (12 bytes), version u32 = 1, `began_ns` i64 (wall clock when the file was made), 8 zero bytes, then the CRC32
//!   of those 32 bytes.
//! - Frame: `len u16` (the body's length), `kind u8`, the body, then `CRC32(kind || body) u32`.
//! - Kinds: 1 `At { version, path }`, 2 `Saved { from, to }`, 3 `Unplaced { path }`, 4 `Gap { reason u8 }`.
//! - A version is flags u8 (bit 0: has a volume), the volume UUID \[16\] (zero when the flag is clear), inode u64, size u64, mtime i64.
//! - A path is a length u16 and then that many bytes: absolute, no NUL, 1 to 4,096 bytes.
//!
//! The ledger holds paths and numbers only: never PDF bytes, text, titles or passwords. A file is hostile input: `read` never panics, caps
//! what it takes before it takes it, stops at the first bad frame, and says why (`Gap`).

use super::{LedgerLimits, Link};
use recto_platform::FileStamp;
use std::path::{Path, PathBuf};

/// The most a ledger file may be. A ledger full at every cap in [`LEDGER_LIMITS`](super::LEDGER_LIMITS) is under it (a test shows it).
pub const MAX_LEDGER_BYTES: usize = 3 << 19;
/// Bytes in the header.
pub const HEADER_LEN: usize = 36;
/// The longest path a frame holds.
pub const MAX_PATH_BYTES: usize = 4_096;

/// 2026-01-01T00:00:00Z in nanoseconds: no ledger began before it.
const EARLIEST_BEGAN_NS: i64 = 1_767_225_600_000_000_000;
const MAGIC: &[u8; 12] = b"RECTOLEDGER\0";
const VERSION: u32 = 1;
/// A frame's fixed bytes around its body: length, kind and checksum.
const FRAME_OVERHEAD: usize = 2 + 1 + 4;
const VERSION_LEN: usize = 1 + 16 + 8 + 8 + 8;
/// The longest body: a version and a path.
const MAX_BODY: usize = VERSION_LEN + 2 + MAX_PATH_BYTES;

/// One version of a file: what a later look at the same file will find equal, whichever mount it is under.
///
/// The volume is its UUID, never `st_dev` (a disk comes back under another device number, and a different disk can get the old one). It
/// is `None` where there is no real one, and a version without one joins nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileVersion {
    pub volume: Option<[u8; 16]>,
    pub inode: u64,
    pub size: u64,
    pub mtime_ns: i64,
}

impl FileVersion {
    /// A stamp seen on the volume `volume`. An all-zero UUID is no UUID.
    pub fn new(stamp: FileStamp, volume: Option<[u8; 16]>) -> Self {
        Self {
            volume: volume.filter(|v| *v != [0; 16]),
            inode: stamp.inode,
            size: stamp.size,
            mtime_ns: stamp.mtime_ns,
        }
    }

    /// What two versions are joined by: the same real volume, a real inode, and the same size and time. `None` joins nothing.
    pub(super) fn join_key(self) -> Option<([u8; 16], u64, u64, i64)> {
        let volume = self.volume.filter(|v| *v != [0; 16])?;
        (self.inode != 0).then_some((volume, self.inode, self.size, self.mtime_ns))
    }
}

/// Why a ledger can't be taken as the whole story. Any gap leaves the removal refusing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Gap {
    /// There is no ledger yet.
    Missing = 1,
    /// A frame or the header is bad, a cap was passed, or the file is not a plain file.
    Damaged,
    /// Written by a newer version, which this one never writes to.
    TooNew,
    /// The last frame was cut short.
    Torn,
    /// The ledger reached a cap and stopped recording.
    Overflowed,
    /// An append failed or timed out.
    WriteFailed,
    /// A save's backup was not under the name the path hashes to.
    BackupMismatch,
    /// Adoption of older journals stopped at its cap.
    AdoptionCut,
    /// One version was saved into two different versions (made by `lineage`, not read from a file).
    Forked,
}

impl Gap {
    fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Self::Missing,
            2 => Self::Damaged,
            3 => Self::TooNew,
            4 => Self::Torn,
            5 => Self::Overflowed,
            6 => Self::WriteFailed,
            7 => Self::BackupMismatch,
            8 => Self::AdoptionCut,
            9 => Self::Forked,
            _ => return None,
        })
    }
}

/// One frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Link(Link),
    Gap(Gap),
}

/// What was read: the links in order, the first reason the whole story is not there, and how much of the file was sound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ledger {
    pub links: Vec<Link>,
    /// When the file was made. `i64::MAX` where it can't be known, so everything reads as older than it.
    pub began_ns: i64,
    /// The first reason the whole story is not there: a `Gap` frame the file holds, or why the read stopped. Not complete if any.
    pub gap: Option<Gap>,
    /// Why the read ended before the end of the file, apart from any `Gap` frames in it: `Torn` (a cut-short tail), `Damaged`, `TooNew`, or
    /// `Missing` for no file. **The writer repairs only on `Some(Torn)`** (cuts back to `good_len`, then appends) **and never writes on
    /// `Damaged` or `TooNew`.** A frame gap such as `Overflowed` is in `gap` and leaves this `None`.
    pub stopped: Option<Gap>,
    /// The bytes that are the header and whole good frames: a torn tail is cut back to here.
    pub good_len: usize,
}

impl Ledger {
    /// No ledger file.
    pub fn missing() -> Self {
        Self::empty(Gap::Missing)
    }

    fn empty(gap: Gap) -> Self {
        Self {
            links: Vec::new(),
            began_ns: i64::MAX,
            gap: Some(gap),
            stopped: Some(gap),
            good_len: 0,
        }
    }
}

/// The header of a new ledger made at `began_ns`.
pub fn encode_header(began_ns: i64) -> [u8; HEADER_LEN] {
    let mut out = [0u8; HEADER_LEN];
    out[..12].copy_from_slice(MAGIC);
    out[12..16].copy_from_slice(&VERSION.to_le_bytes());
    out[16..24].copy_from_slice(&began_ns.to_le_bytes());
    let crc = crc32fast::hash(&out[..32]);
    out[32..].copy_from_slice(&crc.to_le_bytes());
    out
}

/// A frame for `entry`, or `None` for one that could not be read back: a path that is relative, empty, holds a NUL or is over
/// [`MAX_PATH_BYTES`], or a version whose volume flag is set over an all-zero UUID.
pub fn encode(entry: &Entry) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    let kind = match entry {
        Entry::Link(Link::At { version, path }) => {
            put_version(&mut body, *version)?;
            put_path(&mut body, path)?;
            1
        }
        Entry::Link(Link::Saved { from, to }) => {
            put_version(&mut body, *from)?;
            put_version(&mut body, *to)?;
            2
        }
        Entry::Link(Link::Unplaced { path }) => {
            put_path(&mut body, path)?;
            3
        }
        Entry::Gap(gap) => {
            body.push(*gap as u8);
            4
        }
    };
    let mut out = Vec::with_capacity(body.len() + FRAME_OVERHEAD);
    out.extend_from_slice(&u16::try_from(body.len()).ok()?.to_le_bytes());
    out.push(kind);
    out.extend_from_slice(&body);
    out.extend_from_slice(&checksum(kind, &body).to_le_bytes());
    Some(out)
}

fn checksum(kind: u8, body: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&[kind]);
    hasher.update(body);
    hasher.finalize()
}

fn put_version(out: &mut Vec<u8>, v: FileVersion) -> Option<()> {
    if v.volume == Some([0; 16]) {
        return None;
    }
    out.push(u8::from(v.volume.is_some()));
    out.extend_from_slice(&v.volume.unwrap_or([0; 16]));
    out.extend_from_slice(&v.inode.to_le_bytes());
    out.extend_from_slice(&v.size.to_le_bytes());
    out.extend_from_slice(&v.mtime_ns.to_le_bytes());
    Some(())
}

fn put_path(out: &mut Vec<u8>, path: &Path) -> Option<()> {
    let bytes = path.as_os_str().as_encoded_bytes();
    if !valid_path(path) {
        return None;
    }
    out.extend_from_slice(&u16::try_from(bytes.len()).ok()?.to_le_bytes());
    out.extend_from_slice(bytes);
    Some(())
}

fn valid_path(path: &Path) -> bool {
    let bytes = path.as_os_str().as_encoded_bytes();
    (1..=MAX_PATH_BYTES).contains(&bytes.len()) && path.is_absolute() && !bytes.contains(&0)
}

/// A cursor over one frame's body that never reads past it.
struct Body<'a>(&'a [u8]);

impl<'a> Body<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.0.split_at_checked(n)?;
        self.0 = rest;
        Some(head)
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }

    fn version(&mut self) -> Option<FileVersion> {
        let flags = self.take(1)?[0];
        let uuid: [u8; 16] = self.array()?;
        let inode = u64::from_le_bytes(self.array()?);
        let size = u64::from_le_bytes(self.array()?);
        let mtime_ns = i64::from_le_bytes(self.array()?);
        let volume = match flags {
            0 if uuid == [0; 16] => None,
            1 if uuid != [0; 16] => Some(uuid),
            _ => return None,
        };
        Some(FileVersion { volume, inode, size, mtime_ns })
    }

    /// A path no longer than `longest`.
    fn path(&mut self, longest: usize) -> Option<PathBuf> {
        let len = usize::from(u16::from_le_bytes(self.array()?));
        if len > longest {
            return None;
        }
        let path = path_of(self.take(len)?)?;
        valid_path(&path).then_some(path)
    }

    fn done(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(unix)]
fn path_of(bytes: &[u8]) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

#[cfg(not(unix))]
fn path_of(bytes: &[u8]) -> Option<PathBuf> {
    std::str::from_utf8(bytes).ok().map(PathBuf::from)
}

fn decode(kind: u8, body: &[u8], limits: &LedgerLimits) -> Option<Entry> {
    let mut b = Body(body);
    let longest = (limits.max_path_bytes as usize).min(MAX_PATH_BYTES);
    let entry = match kind {
        1 => Entry::Link(Link::At {
            version: b.version()?,
            path: b.path(longest)?,
        }),
        2 => Entry::Link(Link::Saved {
            from: b.version()?,
            to: b.version()?,
        }),
        3 => Entry::Link(Link::Unplaced { path: b.path(longest)? }),
        4 => Entry::Gap(Gap::from_code(b.take(1)?[0])?),
        _ => return None,
    };
    b.done().then_some(entry)
}

/// Whether a frame of this `kind` can have a body of `len` bytes. Checked before the frame's own bounds, so a length flipped mid-file is
/// damage, not a tail cut short: a `Saved` is exactly two versions, a `Gap` one byte, and an `At` or `Unplaced` is its fixed part and its
/// path, whose own length (when the bytes holding it are there) must agree.
fn length_fits(kind: u8, len: usize, frame: &[u8]) -> bool {
    let path_len_at = |at: usize| frame.get(3 + at..3 + at + 2).map(|b| usize::from(u16::from_le_bytes([b[0], b[1]])));
    match kind {
        2 => len == 2 * VERSION_LEN,
        4 => len == 1,
        1 => len > VERSION_LEN + 2 && len <= MAX_BODY && path_len_at(VERSION_LEN).is_none_or(|p| len == VERSION_LEN + 2 + p),
        3 => len > 2 && len <= MAX_BODY - VERSION_LEN && path_len_at(0).is_none_or(|p| len == 2 + p),
        _ => false,
    }
}

/// Reads a ledger, stopping at the first bad frame.
///
/// A file over [`MAX_LEDGER_BYTES`], a bad header or a frame that fails its checks inside the file is `Damaged`; a header from a newer
/// version is `TooNew`; a last frame that is cut short, or whose checksum fails with nothing after it, is `Torn`. More frames than
/// `limits.max_links`, a path over `limits.max_path_bytes` or paths over `limits.max_total_path_bytes` are `Damaged`. What was sound before
/// the bad place is kept, and `good_len` says how far it reaches. Never panics, and holds no more than `limits` of what it reads.
pub fn read(bytes: &[u8], limits: &LedgerLimits) -> Ledger {
    if bytes.len() > MAX_LEDGER_BYTES {
        return Ledger::empty(Gap::Damaged);
    }
    let Some(header) = bytes.get(..HEADER_LEN) else {
        // A header being written is cut short, so it is a prefix of what it will be; anything else is not a ledger.
        let seen = bytes.len().min(MAGIC.len());
        return Ledger::empty(if bytes[..seen] == MAGIC[..seen] { Gap::Torn } else { Gap::Damaged });
    };
    let version = u32::from_le_bytes([header[12], header[13], header[14], header[15]]);
    if &header[..12] != MAGIC {
        return Ledger::empty(Gap::Damaged);
    }
    if version > VERSION {
        return Ledger::empty(Gap::TooNew);
    }
    let stored = u32::from_le_bytes([header[32], header[33], header[34], header[35]]);
    if version != VERSION || crc32fast::hash(&header[..32]) != stored || header[24..32] != [0; 8] {
        return Ledger::empty(Gap::Damaged);
    }
    let began = i64::from_le_bytes([header[16], header[17], header[18], header[19], header[20], header[21], header[22], header[23]]);
    // A time before this code existed was not written by it: every backup then reads as older than the ledger.
    let began_ns = if began < EARLIEST_BEGAN_NS { i64::MAX } else { began };
    let mut out = Ledger {
        links: Vec::new(),
        began_ns,
        gap: None,
        stopped: None,
        good_len: HEADER_LEN,
    };
    let (mut frames, mut path_total) = (0usize, 0usize);
    let mut pos = HEADER_LEN;
    while pos < bytes.len() {
        let rest = &bytes[pos..];
        let Some(head) = rest.get(..3) else {
            return stop(out, Gap::Torn);
        };
        let len = usize::from(u16::from_le_bytes([head[0], head[1]]));
        if !length_fits(head[2], len, rest) {
            return stop(out, Gap::Damaged);
        }
        let end = 3 + len + 4;
        let Some(frame) = rest.get(..end) else {
            return stop(out, Gap::Torn);
        };
        let (kind, body) = (frame[2], &frame[3..3 + len]);
        let stored = u32::from_le_bytes([frame[end - 4], frame[end - 3], frame[end - 2], frame[end - 1]]);
        if checksum(kind, body) != stored {
            return stop(out, if end == rest.len() { Gap::Torn } else { Gap::Damaged });
        }
        frames += 1;
        if frames > limits.max_links as usize {
            return stop(out, Gap::Damaged);
        }
        let Some(entry) = decode(kind, body, limits) else {
            return stop(out, Gap::Damaged);
        };
        match entry {
            Entry::Gap(gap) => out.gap = out.gap.or(Some(gap)),
            Entry::Link(link) => {
                if let Link::At { path, .. } | Link::Unplaced { path } = &link {
                    path_total += path.as_os_str().as_encoded_bytes().len();
                    if path_total > limits.max_total_path_bytes as usize {
                        return stop(out, Gap::Damaged);
                    }
                }
                out.links.push(link);
            }
        }
        pos += end;
        out.good_len = pos;
    }
    out
}

fn stop(mut ledger: Ledger, gap: Gap) -> Ledger {
    ledger.stopped = Some(gap);
    ledger.gap = ledger.gap.or(Some(gap));
    ledger
}

#[cfg(test)]
mod tests;
