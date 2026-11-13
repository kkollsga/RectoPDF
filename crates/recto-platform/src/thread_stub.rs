//! Mach's thread information is the Mac's: elsewhere nothing is seen of another thread.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreadPort(u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub cpu_ms: u64,
    pub state: RunState,
}

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
        "other"
    }
}

impl ThreadPort {
    #[must_use]
    pub fn current() -> Self {
        Self(0)
    }
    #[must_use]
    pub fn sample(self) -> Option<Sample> {
        None
    }
    #[must_use]
    pub fn raw(self) -> u32 {
        self.0
    }
    #[must_use]
    pub fn from_raw(_raw: u32) -> Option<Self> {
        None
    }
}
