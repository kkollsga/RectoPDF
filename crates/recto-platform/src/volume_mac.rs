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
//! The same UUID came from asking the file's path and from asking `f_mntonname`. A volume attached from a copy has the original's UUID, a
//! block copy being a copy of it, which is why the lineage has a fork rule (`docs/design/removal.md` §10, D2b).

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// What the filesystem types that make a volume UUID of their own are called in `f_fstypename`.
const TYPES_WITH_A_REAL_UUID: [&[u8]; 2] = [b"apfs", b"hfs"];

/// What `statfs` and `getattrlist` say about the volume `path` is on: its type name, mount point and UUID.
pub(crate) struct Seen {
    pub(crate) fs_type: Vec<u8>,
    pub(crate) mount_point: Vec<u8>,
    pub(crate) fsid: [i32; 2],
    pub(crate) uuid: Option<[u8; 16]>,
}

#[repr(C, packed(4))]
struct UuidReply {
    length: u32,
    uuid: [u8; 16],
}

fn c_path(bytes: &[u8]) -> Option<CString> {
    CString::new(bytes).ok()
}

fn name_of(field: &[libc::c_char]) -> Vec<u8> {
    field.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect()
}

/// The UUID of the volume holding `path` (a file or a folder), asked of the path itself.
fn uuid_at(path: &CString) -> Option<[u8; 16]> {
    // SAFETY: an all-zero `attrlist` is a valid request for nothing; the fields that matter are set next.
    let mut list: libc::attrlist = unsafe { std::mem::zeroed() };
    list.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    list.volattr = libc::ATTR_VOL_INFO | libc::ATTR_VOL_UUID;
    let mut reply = UuidReply { length: 0, uuid: [0; 16] };
    // SAFETY: `path` is NUL-terminated, `list` is filled in, and the kernel writes at most `size_of::<UuidReply>()` bytes into `reply`.
    let rc = unsafe {
        libc::getattrlist(
            path.as_ptr(),
            (&raw mut list).cast(),
            (&raw mut reply).cast(),
            std::mem::size_of::<UuidReply>(),
            0,
        )
    };
    let length = reply.length;
    let uuid = reply.uuid;
    (rc == 0 && length as usize >= std::mem::size_of::<UuidReply>()).then_some(uuid)
}

pub(crate) fn look(path: &Path) -> Option<Seen> {
    let c = c_path(path.as_os_str().as_bytes())?;
    // SAFETY: an all-zero `statfs` is plain data the kernel then fills.
    let mut fs: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is NUL-terminated and `fs` is a whole `statfs`.
    if unsafe { libc::statfs(c.as_ptr(), &raw mut fs) } != 0 {
        return None;
    }
    let fsid = {
        // `fsid_t` is two i32s, read through its bytes so the layout is not assumed to have named fields.
        // SAFETY: `fsid_t` is `[i32; 2]` in memory.
        unsafe { std::mem::transmute::<libc::fsid_t, [i32; 2]>(fs.f_fsid) }
    };
    Some(Seen {
        fs_type: name_of(&fs.f_fstypename),
        mount_point: name_of(&fs.f_mntonname),
        fsid,
        uuid: uuid_at(&c),
    })
}

/// The identity of the volume `path` is on, or `None` when it has no real one: a filesystem type that makes no UUID of its own, an
/// all-zero answer, or a path that can't be looked at. Only `stat`-class calls: no file is opened.
pub fn volume_identity(path: &Path) -> Option<[u8; 16]> {
    let seen = look(path)?;
    let uuid = seen.uuid?;
    (TYPES_WITH_A_REAL_UUID.contains(&seen.fs_type.as_slice()) && uuid != [0; 16]).then_some(uuid)
}

#[cfg(test)]
#[path = "volume_mac/tests.rs"]
mod tests;
