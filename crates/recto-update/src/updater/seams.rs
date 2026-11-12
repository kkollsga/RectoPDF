//! **What the updater is given, so a test can give it something else** (step R1.4): the wall clock its remembered times go by, and where releases come from and
//! how one is made ready beside the app. The real ones are the defaults, and a release only ever has those: [`Seams::default`]. A test of the app with no window
//! (`recto-app`'s headless kit) gives a clock that moves only when the test moves it, and releases that are only words, so an update's every rule runs in a
//! process of its own in milliseconds, with no server, no zip and no signature.
//!
//! **Why a wall clock, and not the app's own.** What the updater remembers (the last check, the run being watched, when an install was tried) is written to
//! `updates.json` and read by the next launch, so it's the time of day. The app's clock counts from its own start and stops while the Mac sleeps, which would put
//! the daily check off by every night's sleep.

use super::{Host, Look, St, Status, Updater};
use crate::fetch::{self, FetchError};
use crate::plan::Files;
use crate::rules::{Latest, MAX_LATEST_BYTES, parse_latest};
use crate::saved::Saved;
use crate::source::Mode;
use crate::source::Source;
use crate::stage::{self, Stage, StageError, Staged};
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

/// The time of day, in milliseconds since 1970. Reading it is cheap.
pub trait WallClock: Send + Sync {
    fn wall_ms(&self) -> u64;
}

/// The system's.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemWall;

impl WallClock for SystemWall {
    fn wall_ms(&self) -> u64 {
        crate::wall_ms()
    }
}

/// **Where releases come from, and how one's made ready.** Each of these blocks on the network or the disk for seconds, so only ever on a check's or an install's
/// own thread, as they always were.
pub trait Releases: Send + Sync {
    /// The release on offer, as its `latest.json` says, and only what passed.
    ///
    /// # Errors
    ///
    /// It couldn't be fetched or read.
    fn latest(&self, source: &Source, user_agent: &str) -> Result<Latest, FetchError>;

    /// Downloads, checks and unpacks a release beside the app: [`stage::prepare`].
    ///
    /// # Errors
    ///
    /// Why it isn't one to install, in our own words.
    fn stage(&self, stage: &Stage<'_>, say: &mut dyn FnMut(&str)) -> Result<Staged, StageError>;

    /// A development build handed over, checked and unpacked beside the app: [`stage::prepare_build`].
    ///
    /// # Errors
    ///
    /// Why it isn't one to install, in our own words.
    fn stage_build(&self, zip: &mut std::fs::File, version: &str, source: &Source, app: &Path, say: &mut dyn FnMut(&str)) -> Result<Staged, StageError>;
}

/// The releases on the server, downloaded, checked and unpacked for real.
#[derive(Clone, Copy, Debug, Default)]
pub struct RealReleases;

impl Releases for RealReleases {
    fn latest(&self, source: &Source, user_agent: &str) -> Result<Latest, FetchError> {
        let body = fetch::get_small(source, &source.latest_url(), user_agent, MAX_LATEST_BYTES, fetch::CHECK_LIMIT)?;
        parse_latest(&body).ok_or(FetchError::Failed("the release's description can't be read"))
    }

    fn stage(&self, stage: &Stage<'_>, say: &mut dyn FnMut(&str)) -> Result<Staged, StageError> {
        stage::prepare(stage, say)
    }

    fn stage_build(&self, zip: &mut std::fs::File, version: &str, source: &Source, app: &Path, say: &mut dyn FnMut(&str)) -> Result<Staged, StageError> {
        stage::prepare_build(zip, version, source, app, say)
    }
}

/// What an updater is made with, past its state folder, its source and its host.
#[derive(Clone)]
pub struct Seams {
    pub wall: Arc<dyn WallClock>,
    pub releases: Arc<dyn Releases>,
}

impl Default for Seams {
    /// The real ones: the system's clock and the release server.
    fn default() -> Self {
        Self {
            wall: Arc::new(SystemWall),
            releases: Arc::new(RealReleases),
        }
    }
}

/// **The updater made, and its clocks:** the time of day it goes by, and a test's offset on it.
impl Updater {
    #[must_use]
    pub fn new(root: &Path, source: Source, host: Arc<dyn Host>) -> Arc<Self> {
        Self::with_seams(root, source, host, Seams::default())
    }

    /// [`Self::new`], with a clock and releases of the caller's: a test's.
    #[must_use]
    pub fn with_seams(root: &Path, source: Source, host: Arc<dyn Host>, seams: Seams) -> Arc<Self> {
        let st = St {
            status: Status::Idle,
            saved: Saved::default(),
            launched_ms: seams.wall.wall_ms(),
            facts: None,
            second_copy: false,
            notice: None,
            install_look_at: None,
            arrived: None,
            reopened_by_update: false,
            fall_back: None,
            first_frame_done: false,
            first_frame_early: None,
            build: None,
            from_menu: false,
            alert: None,
        };
        Arc::new(Self {
            files: Files::in_state(root),
            source,
            host,
            st: Mutex::new(st),
            writing: Mutex::new(()),
            clock_ahead_ms: AtomicI64::new(0),
            stopping: AtomicBool::new(false),
            seams,
        })
    }

    /// The time of day by this updater's wall clock, without a test's offset: what a first frame and a quit are stamped with, as the installer reads them.
    #[must_use]
    pub fn wall_ms(&self) -> u64 {
        self.seams.wall.wall_ms()
    }

    /// Now, by this updater's clock.
    #[must_use]
    pub fn now_ms(&self) -> u64 {
        self.wall_ms().saturating_add_signed(self.clock_ahead_ms.load(Ordering::SeqCst))
    }

    /// Moves a test's clock on. Says whether it did: only a test run's clock moves.
    pub fn set_test_clock(&self, ahead_ms: i64) -> bool {
        if self.source.mode != Mode::Test {
            return false;
        }
        self.clock_ahead_ms.store(ahead_ms, Ordering::SeqCst);
        true
    }

    pub(super) fn since_input(&self, look: &Look) -> u64 {
        look.since_input_ms.saturating_add_signed(self.clock_ahead_ms.load(Ordering::SeqCst))
    }
}

/// A limit, in the milliseconds the wall clock counts.
pub(super) fn duration_ms(limit: Duration) -> u64 {
    u64::try_from(limit.as_millis()).unwrap_or(u64::MAX)
}
