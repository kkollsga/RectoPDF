//! Which volume a file is on, by the volume's own UUID (`getattrlist`, `ATTR_VOL_INFO | ATTR_VOL_UUID`), not by the mount's device number.
//!
//! `st_dev` belongs to a mount: an ejected volume comes back under another number and a later disk can get the old one. The UUID belongs
//! to the volume. Only a filesystem that makes a real one counts: `apfs` and `hfs`. FAT and exFAT make theirs from a 32-bit serial number
//! and network volumes have none, so those, and an all-zero answer, are `None` and join nothing by version.
//!
//! **Measured, not assumed** (guide §6.2). Probe: `cargo test -p recto-platform volume_probe -- --ignored --nocapture`.
//!
//! Measured 2026-11-26 on macOS 27.0.1 (26A434), by the probe in `volume_mac/tests.rs`, 20 MiB images attached with
//! `-nobrowse -noautoopen -mountpoint`: attach A, detach, attach A again, `cp` A to B, attach B alone, attach A and B together. Per
//! filesystem (UUIDs shortened to their first 8 hex digits; "same" means equal on every step):
//!
//! ```text
//! fs      f_fstypename  UUID                         st_dev / f_fsid.val[0]                    inode  A vs B
//! APFS    apfs          d4f23fa1.. same on all steps A 16777248 on attach, re-attach and B alone;  16     B has A's UUID, also with both attached
//! HFS+    hfs           31a59e8d.. same on all steps A 16777246 likewise                           18     same
//! ExFAT   exfat         87c61902.. same on all steps (counted: no)                                  6     same
//! MS-DOS  msdos         27503b4b.. same (counted: no)                                               2     B got another UUID when both attached
//! ```
//!
//! With A and B attached together, B's `st_dev` was another number (16777252 for APFS, 16777248 for HFS+, whose number A's earlier attach
//! had been given): `st_dev` and `f_fsid.val[0]` are equal, and the number is reused by the next attach, so it can't say which disk it is.
//! ExFAT is left out though its UUID looked stable here: it is made from a 32-bit volume serial number, not a UUID of its own, so two
//! cards can share one (MS-DOS likewise, and its copy was given a different one beside the original). Leaving them out can only cause a
//! false refusal, never a false join.
//! Re-run 2026-11-26 after the type name moved into the same `getattrlist` call as the UUID (`ATTR_VOL_FSTYPENAME`, so both come from one
//! mount): the same shape on every step (UUID stable across re-attach and `cp`, equal beside the original on APFS and HFS+, `getattrlist`'s
//! type equal to `statfs`'s on all four), with fresh UUIDs because the images were new.
//! The same UUID came from asking the file's path and from asking `f_mntonname`. A volume attached from a copy has the original's UUID, a
//! block copy being a copy of it, which is why the lineage has a fork rule (`docs/design/removal.md` §10, D2b).

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// What the filesystem types that make a volume UUID of their own are called (`f_fstypename`, `ATTR_VOL_FSTYPENAME`).
const TYPES_WITH_A_REAL_UUID: [&[u8]; 2] = [b"apfs", b"hfs"];

/// What one `getattrlist` says about the volume `path` is on: its filesystem type name and UUID, from the same call so they can't come from
/// two different mounts.
pub(crate) struct Volume {
    pub(crate) fs_type: Vec<u8>,
    pub(crate) uuid: [u8; 16],
}

/// The reply's length (u32), then the UUID (16 bytes), then the type name's `attrreference_t` (offset from itself, i32, and length with its
/// NUL, u32), whose string follows in the buffer. 256 bytes hold a name of any filesystem.
const REPLY_BYTES: usize = 256;

pub(crate) fn volume_of(path: &Path) -> Option<Volume> {
    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: an all-zero `attrlist` is a valid request for nothing; the fields that matter are set next.
    let mut list: libc::attrlist = unsafe { std::mem::zeroed() };
    list.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    list.volattr = libc::ATTR_VOL_INFO | libc::ATTR_VOL_UUID | ATTR_VOL_FSTYPENAME;
    let mut reply = [0u8; REPLY_BYTES];
    // SAFETY: `path` is NUL-terminated, `list` is filled in, and the kernel writes at most `REPLY_BYTES` bytes into `reply`.
    let rc = unsafe { libc::getattrlist(path.as_ptr(), (&raw mut list).cast(), reply.as_mut_ptr().cast(), REPLY_BYTES, 0) };
    if rc != 0 {
        return None;
    }
    let word = |at: usize| Some([*reply.get(at)?, *reply.get(at + 1)?, *reply.get(at + 2)?, *reply.get(at + 3)?]);
    let length = usize::try_from(u32::from_le_bytes(word(0)?)).ok()?.min(REPLY_BYTES);
    let uuid: [u8; 16] = reply.get(4..20)?.try_into().ok()?;
    let (offset, size) = (i32::from_le_bytes(word(20)?), usize::try_from(u32::from_le_bytes(word(24)?)).ok()?);
    let start = usize::try_from(20i64 + i64::from(offset)).ok()?;
    let name = reply.get(start..start.checked_add(size)?)?;
    (start + size <= length).then_some(())?;
    let end = name.iter().position(|b| *b == 0).unwrap_or(name.len());
    Some(Volume {
        fs_type: name[..end].to_vec(),
        uuid,
    })
}

/// `ATTR_VOL_FSTYPENAME`, which `libc` doesn't name yet (`sys/attr.h`).
const ATTR_VOL_FSTYPENAME: libc::attrgroup_t = 0x0010_0000;

/// What `statfs` says of the mount, for the probe only: its device and id numbers, where it is mounted and its flags.
#[cfg(test)]
pub(crate) struct Mount {
    pub(crate) mount_point: Vec<u8>,
    pub(crate) fsid: [i32; 2],
    pub(crate) fs_type: Vec<u8>,
    /// `f_flags`: the mount's flags (`MNT_IGNORE_OWNERSHIP` is set when it was mounted with ownership off).
    pub(crate) flags: u32,
}

#[cfg(test)]
pub(crate) fn mount_of(path: &Path) -> Option<Mount> {
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: an all-zero `statfs` is plain data the kernel then fills.
    let mut fs: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is NUL-terminated and `fs` is a whole `statfs`.
    if unsafe { libc::statfs(c.as_ptr(), &raw mut fs) } != 0 {
        return None;
    }
    let name = |field: &[libc::c_char]| field.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect::<Vec<u8>>();
    // SAFETY: `fsid_t` is `[i32; 2]` in memory.
    let fsid = unsafe { std::mem::transmute::<libc::fsid_t, [i32; 2]>(fs.f_fsid) };
    Some(Mount {
        mount_point: name(&fs.f_mntonname),
        fsid,
        fs_type: name(&fs.f_fstypename),
        flags: fs.f_flags,
    })
}

/// The identity of the volume `path` is on, or `None` when it has no real one: a filesystem type that makes no UUID of its own, an
/// all-zero answer, or a path that can't be looked at. One `getattrlist`; no file is opened.
pub fn volume_identity(path: &Path) -> Option<[u8; 16]> {
    let seen = volume_of(path)?;
    (TYPES_WITH_A_REAL_UUID.contains(&seen.fs_type.as_slice()) && seen.uuid != [0; 16]).then_some(seen.uuid)
}

#[cfg(test)]
#[path = "volume_mac/tests.rs"]
mod tests;
