//! What the window server says is on screen, for tests that prove a quiet run shows nothing. CoreGraphics is opened only when this is asked, so nothing that never asks links it or loads it.

use std::ffi::{CStr, c_void};
use std::sync::OnceLock;

type CfRef = *const c_void;

/// A window on a screen just now: whose it is, its number and its frame in the window server's points, from the top left of the main screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShownWindow {
    pub pid: u32,
    pub number: i64,
    pub frame: [f64; 4],
}

#[repr(C)]
#[derive(Default)]
struct CgRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFArrayGetCount(array: CfRef) -> isize;
    fn CFArrayGetValueAtIndex(array: CfRef, index: isize) -> CfRef;
    fn CFDictionaryGetValue(dict: CfRef, key: CfRef) -> CfRef;
    fn CFNumberGetValue(number: CfRef, kind: isize, value: *mut c_void) -> bool;
    fn CFRelease(value: CfRef);
}

const ON_SCREEN_ONLY: u32 = 1 << 0;
const EXCLUDE_DESKTOP: u32 = 1 << 4;
const CF_NUMBER_SINT64: isize = 4;

struct Graphics {
    list: unsafe extern "C" fn(u32, u32) -> CfRef,
    rect: unsafe extern "C" fn(CfRef, *mut CgRect) -> bool,
    owner_pid: CfRef,
    number: CfRef,
    bounds: CfRef,
}

// SAFETY: the keys are immutable CFString constants of a framework that's never unloaded, and the functions are thread-safe.
unsafe impl Send for Graphics {}
// SAFETY: as above.
unsafe impl Sync for Graphics {}

fn graphics() -> Option<&'static Graphics> {
    static LOADED: OnceLock<Option<Graphics>> = OnceLock::new();
    LOADED
        .get_or_init(|| {
            const PATH: &CStr = c"/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics";
            // SAFETY: a system framework by its full path, kept open for the life of the process.
            let handle = unsafe { libc::dlopen(PATH.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
            if handle.is_null() {
                return None;
            }
            let find = |name: &CStr| {
                // SAFETY: a symbol of the handle opened above.
                let found = unsafe { libc::dlsym(handle, name.as_ptr()) };
                (!found.is_null()).then_some(found)
            };
            // SAFETY: each constant's symbol is the address of a CFStringRef, read once.
            let key = |name: &CStr| find(name).map(|found| unsafe { *found.cast::<CfRef>() });
            let list = find(c"CGWindowListCopyWindowInfo")?;
            let rect = find(c"CGRectMakeWithDictionaryRepresentation")?;
            Some(Graphics {
                // SAFETY: these are the functions' documented signatures.
                list: unsafe { std::mem::transmute::<*mut c_void, unsafe extern "C" fn(u32, u32) -> CfRef>(list) },
                // SAFETY: as above.
                rect: unsafe { std::mem::transmute::<*mut c_void, unsafe extern "C" fn(CfRef, *mut CgRect) -> bool>(rect) },
                owner_pid: key(c"kCGWindowOwnerPID")?,
                number: key(c"kCGWindowNumber")?,
                bounds: key(c"kCGWindowBounds")?,
            })
        })
        .as_ref()
}

fn number(dict: CfRef, key: CfRef) -> Option<i64> {
    // SAFETY: `dict` is a live CFDictionary and `key` a CFString. What comes back is borrowed from the dictionary.
    let value = unsafe { CFDictionaryGetValue(dict, key) };
    let mut out = 0i64;
    // SAFETY: the value is a CFNumber when it's there, and `out` an i64 as the kind says.
    (!value.is_null() && unsafe { CFNumberGetValue(value, CF_NUMBER_SINT64, (&raw mut out).cast()) }).then_some(out)
}

/// Every window on any screen just now, the desktop's aside. None if the window server can't be asked.
pub fn windows_on_screen() -> Option<Vec<ShownWindow>> {
    let graphics = graphics()?;
    // SAFETY: no window to be relative to. The array is ours, and released below.
    let list = unsafe { (graphics.list)(ON_SCREEN_ONLY | EXCLUDE_DESKTOP, 0) };
    if list.is_null() {
        return None;
    }
    let mut shown = Vec::new();
    // SAFETY: `list` is a live CFArray of CFDictionaries until it's released.
    for index in 0..unsafe { CFArrayGetCount(list) } {
        // SAFETY: as above, with the index in range.
        let dict = unsafe { CFArrayGetValueAtIndex(list, index) };
        let Some(pid) = number(dict, graphics.owner_pid).and_then(|pid| u32::try_from(pid).ok()) else { continue };
        let mut rect = CgRect::default();
        // SAFETY: a dictionary of the window's, borrowed, or null.
        let bounds = unsafe { CFDictionaryGetValue(dict, graphics.bounds) };
        if !bounds.is_null() {
            // SAFETY: `bounds` is the window's bounds dictionary, and `rect` is written by the call.
            unsafe { (graphics.rect)(bounds, &raw mut rect) };
        }
        shown.push(ShownWindow { pid, number: number(dict, graphics.number).unwrap_or(0), frame: [rect.x, rect.y, rect.width, rect.height] });
    }
    // SAFETY: copied above, released once.
    unsafe { CFRelease(list) };
    Some(shown)
}

#[cfg(test)]
mod tests {
    /// Failure: the window server's list can't be read through what's opened, or its owners and frames come back as nonsense, so a test that a quiet run shows nothing passes having looked at nothing.
    #[test]
    fn the_window_server_can_be_asked() {
        let Some(shown) = super::windows_on_screen() else {
            // A machine with no window server, as a build host over SSH may be, says so rather than nothing.
            eprintln!("skipped: no window server here");
            return;
        };
        // A list that's never empty where there's a screen: the menu bar is a window. A session with no screen lists nothing, and the app tests say they've looked at nothing there.
        if shown.is_empty() {
            eprintln!("skipped: nothing on any screen here");
            return;
        }
        assert!(shown.iter().all(|window| window.pid > 0 && window.frame[2] >= 0.0 && window.frame[3] >= 0.0), "{shown:?}");
    }
}
