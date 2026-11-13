//! **What a thread is doing, seen from another one** (Mach `thread_info`, `THREAD_BASIC_INFO`): its CPU time and whether it's running or waiting. It's how a stall of the UI thread tells waiting (blocked on something) from starved (runnable, not given a core) without `sample` (CI, 2026-11-13: a 2.3 s stall whose `sample` didn't end within its bound).

/// A thread's port. The right is kept for the life of the process: one per thread asked about, once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreadPort(u32);

/// Where a thread was at a look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    /// User and system time together, in ms.
    pub cpu_ms: u64,
    pub state: RunState,
}

/// The kernel's account of a thread. `Running` is on a core or runnable: with CPU time not moving, that is starved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    Running,
    Waiting,
    Uninterruptible,
    Stopped,
    Other,
}

impl RunState {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Running => "running_or_runnable",
            Self::Waiting => "waiting",
            Self::Uninterruptible => "uninterruptible",
            Self::Stopped => "stopped",
            Self::Other => "other",
        }
    }
}

impl ThreadPort {
    /// The calling thread's own port.
    #[must_use]
    pub fn current() -> Self {
        // The same as the one `isolate_mac` uses for the task's: libc marks them deprecated for `mach2`, which this one call doesn't justify a crate.
        #[allow(deprecated)]
        let port = unsafe { libc::mach_thread_self() };
        Self(port)
    }

    /// The thread's CPU time and run state now, or `None` if the kernel won't say (the thread's gone).
    #[must_use]
    pub fn sample(self) -> Option<Sample> {
        let mut info: libc::thread_basic_info = unsafe { std::mem::zeroed() };
        let mut count = libc::THREAD_BASIC_INFO_COUNT;
        let rc = unsafe { libc::thread_info(self.0, libc::THREAD_BASIC_INFO as libc::thread_flavor_t, (&raw mut info).cast(), &raw mut count) };
        if rc != 0 {
            return None;
        }
        let ms = |time: libc::time_value_t| u64::try_from(time.seconds).unwrap_or(0) * 1000 + u64::try_from(time.microseconds).unwrap_or(0) / 1000;
        let state = match info.run_state {
            libc::TH_STATE_RUNNING => RunState::Running,
            libc::TH_STATE_WAITING => RunState::Waiting,
            libc::TH_STATE_UNINTERRUPTIBLE => RunState::Uninterruptible,
            libc::TH_STATE_STOPPED | libc::TH_STATE_HALTED => RunState::Stopped,
            _ => RunState::Other,
        };
        Some(Sample {
            cpu_ms: ms(info.user_time) + ms(info.system_time),
            state,
        })
    }

    /// The port as a number, to keep in an atomic. Zero is no port.
    #[must_use]
    pub fn raw(self) -> u32 {
        self.0
    }

    #[must_use]
    pub fn from_raw(raw: u32) -> Option<Self> {
        (raw != 0).then_some(Self(raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: the sample can't tell a thread that's working from one that's blocked, which is all it is for.
    #[test]
    fn a_busy_thread_runs_and_a_blocked_one_waits() {
        let (port_tx, port_rx) = std::sync::mpsc::channel();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let blocked = std::thread::spawn(move || {
            port_tx.send(ThreadPort::current().raw()).unwrap();
            let _ = stop_rx.recv();
        });
        let port = ThreadPort::from_raw(port_rx.recv().unwrap()).unwrap();
        let mut state = None;
        for _ in 0..200 {
            state = port.sample().map(|sample| sample.state);
            if state == Some(RunState::Waiting) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5)); // ratchet: poll
        }
        assert_eq!(state, Some(RunState::Waiting));
        stop_tx.send(()).unwrap();
        blocked.join().unwrap();
        // This thread, spinning: CPU time moves, and it is running while it looks at itself.
        let me = ThreadPort::current();
        let before = me.sample().unwrap();
        let began = std::time::Instant::now();
        while began.elapsed() < std::time::Duration::from_millis(60) {
            std::hint::spin_loop();
        }
        let after = me.sample().unwrap();
        assert_eq!(after.state, RunState::Running);
        assert!(after.cpu_ms > before.cpu_ms, "{before:?} {after:?}");
    }
}
