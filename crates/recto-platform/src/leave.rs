//! Leaving a process at once, from a thread, whatever the rest of it is doing.
//!
//! `std::process::exit` is not that: it runs the C library's exit handlers and the static destructors of everything linked in, and any of them can wait for a lock or a thread that busy code holds. A helper that has to go when its parent closes its stdin, or when it passes its memory limit, can't wait on that (2026-11-13: the contents helper worked on for 5.3 s after its stdin closed). `leave_now` ends the process in the kernel and runs none of it.

/// Ends this process now with `code`: no exit handler, no destructor, no flush. Say what must get out before calling it.
pub fn leave_now(code: i32) -> ! {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: `_exit` takes an integer, touches no Rust state, and doesn't return.
        unsafe { libc::_exit(code) }
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::process::exit(code)
    }
}

/// Test builds only, compiled out of release: registers a C exit handler that stalls `ms` milliseconds, as a static destructor waiting on a lock does. It makes `std::process::exit` slow and leaves `leave_now` alone, which is what a test of the helper's leaving needs to tell them apart.
#[cfg(all(debug_assertions, target_os = "macos"))]
pub fn stall_exit_handlers(ms: u64) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static STALL_MS: AtomicU64 = AtomicU64::new(0);
    extern "C" fn stall() {
        std::thread::sleep(std::time::Duration::from_millis(STALL_MS.load(Ordering::SeqCst)));
    }
    STALL_MS.store(ms, Ordering::SeqCst);
    // SAFETY: registers a plain function with no state of its own.
    unsafe {
        libc::atexit(stall);
    }
}

#[cfg(not(all(debug_assertions, target_os = "macos")))]
pub fn stall_exit_handlers(_ms: u64) {}
