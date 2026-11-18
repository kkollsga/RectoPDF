//! A process-local exit bound for the test fixture's unsandboxed Escape executable.
//! No borrowed PID, thread, allocation or child is needed: the kernel owns one real-time timer.

use std::io;

pub(super) fn arm(milliseconds: u32) -> io::Result<()> {
    if milliseconds == 0 {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    // SAFETY: these are local signal sets/actions; every syscall result is checked. Restoring the default disposition and unblocking the signal
    // is necessary because an ignored or blocked SIGALRM survives exec. No handler runs app code.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = libc::SIG_DFL;
        if libc::sigemptyset(&raw mut action.sa_mask) != 0 || libc::sigaction(libc::SIGALRM, &raw const action, std::ptr::null_mut()) != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut unblocked: libc::sigset_t = 0;
        if libc::sigemptyset(&raw mut unblocked) != 0
            || libc::sigaddset(&raw mut unblocked, libc::SIGALRM) != 0
            || libc::sigprocmask(libc::SIG_UNBLOCK, &raw const unblocked, std::ptr::null_mut()) != 0
        {
            return Err(io::Error::last_os_error());
        }
        let timer = libc::itimerval {
            it_interval: libc::timeval { tv_sec: 0, tv_usec: 0 },
            it_value: libc::timeval {
                tv_sec: i64::from(milliseconds / 1000),
                // The remainder is at most 999 ms, so this fits the SDK's suseconds_t.
                tv_usec: ((milliseconds % 1000) * 1000) as libc::suseconds_t,
            },
        };
        if libc::setitimer(libc::ITIMER_REAL, &raw const timer, std::ptr::null_mut()) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// The cheap guard's explicit probe-only leaf: it must end by SIGALRM, independently of the collector's later SIGKILL deadline.
pub(super) fn exercise() -> ! {
    if arm(50).is_err() {
        std::process::exit(2);
    }
    loop {
        // SAFETY: pause only blocks this process until a signal arrives; the default SIGALRM action terminates it.
        unsafe { libc::pause() };
    }
}
