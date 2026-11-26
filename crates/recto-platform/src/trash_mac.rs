//! Moving a file or folder to the Trash of the volume it is on, as Finder's "Move to Trash" does and Put Back understands (`docs/design/removal.md` §10, D5).
//!
//! `NSFileManager`'s `trashItemAtURL:resultingItemURL:error:` is the one call that does it: the item is renamed into that volume's own Trash (the person's
//! `~/.Trash` on the startup disk, `.Trashes/<uid>` on another), with its Put Back record, and nothing is copied or deleted. Foundation and the Objective-C runtime
//! are opened here by `dlopen` and called by `objc_msgSend`, the way `find_key_mac` opens Security, so no program that never asks loads this and the crate needs
//! no Objective-C binding. Every call is made inside an autorelease pool of its own, so the objects Foundation returns go when the call does.
//!
//! **No Trash is no deletion.** [`has_trash_for`] asks `URLForDirectory:NSTrashDirectory … create:NO` for the volume of a path, which makes nothing, and
//! accepts a local writable volume that has none yet; a volume that can't (a network share, a read-only or foreign disk) says `false`, and [`move_to_trash`] on it fails and leaves the item where it is. Nothing here ever
//! unlinks anything. What each of these answers on a disk image of its own, and that the item keeps its device and inode through the move, is measured by the
//! `--ignored` probe in `trash_mac/tests.rs`, whose result is written there.

use std::ffi::{CStr, CString, c_void};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

type Id = *mut c_void;
type Sel = *mut c_void;

/// Why an item was not moved to the Trash. Each leaves the item where it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// Foundation or the Objective-C runtime couldn't be opened here.
    Unavailable,
    /// The path can't be given to the system (a NUL in it, or no such thing).
    BadPath,
    /// The system refused: `NSError.code` (`NSFileWriteUnknownError` 512 and its kin).
    Refused(isize),
}

/// The calls, opened once and kept for the life of the process.
struct Objc {
    get_class: unsafe extern "C" fn(*const libc::c_char) -> Id,
    register: unsafe extern "C" fn(*const libc::c_char) -> Sel,
    send: *mut c_void,
    push: unsafe extern "C" fn() -> *mut c_void,
    pop: unsafe extern "C" fn(*mut c_void),
}

// SAFETY: plain function addresses of a runtime that is never unloaded, which may be called from any thread.
unsafe impl Send for Objc {}
// SAFETY: as above.
unsafe impl Sync for Objc {}

fn objc() -> Option<&'static Objc> {
    static LOADED: OnceLock<Option<Objc>> = OnceLock::new();
    LOADED
        .get_or_init(|| {
            const FOUNDATION: &CStr = c"/System/Library/Frameworks/Foundation.framework/Foundation";
            const RUNTIME: &CStr = c"/usr/lib/libobjc.A.dylib";
            // SAFETY: two system libraries by their full paths, kept open for the life of the process.
            let (foundation, runtime) = unsafe {
                (
                    libc::dlopen(FOUNDATION.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL),
                    libc::dlopen(RUNTIME.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL),
                )
            };
            if foundation.is_null() || runtime.is_null() {
                return None;
            }
            // SAFETY: a symbol of the runtime opened above.
            let find = |name: &CStr| {
                let found = unsafe { libc::dlsym(runtime, name.as_ptr()) };
                (!found.is_null()).then_some(found)
            };
            // SAFETY: each is the named function's real signature.
            unsafe {
                Some(Objc {
                    get_class: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*const libc::c_char) -> Id>(find(c"objc_getClass")?),
                    register: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*const libc::c_char) -> Sel>(find(c"sel_registerName")?),
                    send: find(c"objc_msgSend")?,
                    push: std::mem::transmute::<*mut c_void, unsafe extern "C" fn() -> *mut c_void>(find(c"objc_autoreleasePoolPush")?),
                    pop: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut c_void)>(find(c"objc_autoreleasePoolPop")?),
                })
            }
        })
        .as_ref()
}

/// A pool open for one call.
struct Pool {
    objc: &'static Objc,
    token: *mut c_void,
}

impl Pool {
    fn new(objc: &'static Objc) -> Self {
        // SAFETY: the runtime's own function, popped by `drop` on this thread.
        Self {
            objc,
            token: unsafe { (objc.push)() },
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        // SAFETY: the token `push` gave, popped once.
        unsafe { (self.objc.pop)(self.token) };
    }
}

impl Objc {
    fn class(&self, name: &CStr) -> Id {
        // SAFETY: a NUL-ended name.
        unsafe { (self.get_class)(name.as_ptr()) }
    }

    fn sel(&self, name: &CStr) -> Sel {
        // SAFETY: a NUL-ended name.
        unsafe { (self.register)(name.as_ptr()) }
    }
}

/// `+[NSURL fileURLWithFileSystemRepresentation:isDirectory:relativeToURL:]` of `path`, or null.
fn url_of(objc: &Objc, path: &Path) -> Result<Id, Failure> {
    let cpath = CString::new(path.as_os_str().as_bytes()).map_err(|_| Failure::BadPath)?;
    let is_folder = std::fs::symlink_metadata(path).map_err(|_| Failure::BadPath)?.is_dir();
    let class = objc.class(c"NSURL");
    if class.is_null() {
        return Err(Failure::Unavailable);
    }
    // SAFETY: the selector's real signature: (id, SEL, const char *, BOOL, id) -> id.
    let make: unsafe extern "C" fn(Id, Sel, *const libc::c_char, bool, Id) -> Id = unsafe { std::mem::transmute(objc.send) };
    // SAFETY: `class` is NSURL, `cpath` outlives the call, and the base URL is nil.
    let url = unsafe {
        make(
            class,
            objc.sel(c"fileURLWithFileSystemRepresentation:isDirectory:relativeToURL:"),
            cpath.as_ptr(),
            is_folder,
            std::ptr::null_mut(),
        )
    };
    if url.is_null() { Err(Failure::BadPath) } else { Ok(url) }
}

/// `-[NSURL fileSystemRepresentation]` of `url`, copied out.
fn path_of(objc: &Objc, url: Id) -> Option<PathBuf> {
    if url.is_null() {
        return None;
    }
    // SAFETY: the selector's real signature: (id, SEL) -> const char *.
    let rep: unsafe extern "C" fn(Id, Sel) -> *const libc::c_char = unsafe { std::mem::transmute(objc.send) };
    // SAFETY: `url` is an NSURL; the string it returns lives until the pool pops and is copied before.
    let text = unsafe { rep(url, objc.sel(c"fileSystemRepresentation")) };
    // SAFETY: NUL-ended, and read before anything else is sent.
    (!text.is_null()).then(|| PathBuf::from(std::ffi::OsString::from_vec(unsafe { CStr::from_ptr(text) }.to_bytes().to_vec())))
}

fn default_manager(objc: &Objc) -> Result<Id, Failure> {
    let class = objc.class(c"NSFileManager");
    if class.is_null() {
        return Err(Failure::Unavailable);
    }
    // SAFETY: the selector's real signature: (id, SEL) -> id.
    let get: unsafe extern "C" fn(Id, Sel) -> Id = unsafe { std::mem::transmute(objc.send) };
    // SAFETY: `class` is NSFileManager.
    let manager = unsafe { get(class, objc.sel(c"defaultManager")) };
    if manager.is_null() { Err(Failure::Unavailable) } else { Ok(manager) }
}

fn code_of(objc: &Objc, error: Id) -> isize {
    if error.is_null() {
        return 0;
    }
    // SAFETY: the selector's real signature: (id, SEL) -> NSInteger.
    let code: unsafe extern "C" fn(Id, Sel) -> isize = unsafe { std::mem::transmute(objc.send) };
    // SAFETY: `error` is an NSError.
    unsafe { code(error, objc.sel(c"code")) }
}

/// Whether an item at `path` can go to the Trash of its volume: the volume already has one for this user, or it is a local volume mounted writable, where
/// the first move makes it. **Measured (2026-11-26, HFS+ images, `trash_mac/tests.rs`):** `URLForDirectory:NSTrashDirectory … create:NO` answers `false` on a
/// writable volume that has no Trash folder yet (and `create:YES` does too), and `true` once the first item went in, so the question alone would call every
/// fresh external disk "no Trash"; on a read-only volume it is `false` and a move is refused with the item left in place. `false` here means a volume that
/// is read-only, not local, or can't be asked: nothing may be moved, and it is never deleted instead.
pub fn has_trash_for(path: &Path) -> bool {
    asks_for_trash(path, false) || writable_local(path)
}

/// Whether `path` is on a local volume mounted writable (`statfs` flags `MNT_LOCAL` set and `MNT_RDONLY` clear).
fn writable_local(path: &Path) -> bool {
    let Ok(cpath) = CString::new(path.as_os_str().as_bytes()) else { return false };
    // SAFETY: an all-zero `statfs` is plain data the kernel then fills.
    let mut fs: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `cpath` is NUL-terminated and `fs` is a whole `statfs`.
    if unsafe { libc::statfs(cpath.as_ptr(), &raw mut fs) } != 0 {
        return false;
    }
    fs.f_flags & libc::MNT_LOCAL as u32 != 0 && fs.f_flags & libc::MNT_RDONLY as u32 == 0
}

/// `URLForDirectory:NSTrashDirectory` of the volume of `path`, with `create:` as given: whether it answers with a URL.
fn asks_for_trash(path: &Path, create: bool) -> bool {
    let Some(objc) = objc() else { return false };
    let _pool = Pool::new(objc);
    let (Ok(manager), Ok(url)) = (default_manager(objc), url_of(objc, path)) else {
        return false;
    };
    /// `NSTrashDirectory` and `NSUserDomainMask`.
    const TRASH_DIRECTORY: usize = 102;
    const USER_DOMAIN: usize = 1;
    // SAFETY: the selector's real signature: (id, SEL, NSSearchPathDirectory, NSSearchPathDomainMask, id, BOOL, id *) -> id.
    let ask: unsafe extern "C" fn(Id, Sel, usize, usize, Id, bool, *mut Id) -> Id = unsafe { std::mem::transmute(objc.send) };
    let mut error: Id = std::ptr::null_mut();
    // SAFETY: `manager` and `url` are live objects of the pool.
    let found = unsafe {
        ask(
            manager,
            objc.sel(c"URLForDirectory:inDomain:appropriateForURL:create:error:"),
            TRASH_DIRECTORY,
            USER_DOMAIN,
            url,
            create,
            &mut error,
        )
    };
    !found.is_null()
}

/// Moves `path` to the Trash of its own volume, and says where it went. The item is renamed, never copied or deleted: on a failure it is where it was.
/// A folder goes with all it holds; a link goes as itself.
pub fn move_to_trash(path: &Path) -> Result<PathBuf, Failure> {
    let objc = objc().ok_or(Failure::Unavailable)?;
    let _pool = Pool::new(objc);
    let manager = default_manager(objc)?;
    let url = url_of(objc, path)?;
    // SAFETY: the selector's real signature: (id, SEL, id, id *, id *) -> BOOL.
    let trash: unsafe extern "C" fn(Id, Sel, Id, *mut Id, *mut Id) -> bool = unsafe { std::mem::transmute(objc.send) };
    let (mut resulting, mut error): (Id, Id) = (std::ptr::null_mut(), std::ptr::null_mut());
    // SAFETY: `manager` and `url` are live objects of the pool, and both out-parameters are slots of ours.
    let moved = unsafe { trash(manager, objc.sel(c"trashItemAtURL:resultingItemURL:error:"), url, &mut resulting, &mut error) };
    if !moved {
        return Err(Failure::Refused(code_of(objc, error)));
    }
    path_of(objc, resulting).ok_or(Failure::Refused(0))
}

#[cfg(test)]
#[path = "trash_mac/tests.rs"]
mod tests;

/// What the probe measures beside [`has_trash_for`]: the same question with `create:YES`.
#[cfg(test)]
pub(crate) fn has_trash_creating(path: &Path) -> bool {
    asks_for_trash(path, true)
}
