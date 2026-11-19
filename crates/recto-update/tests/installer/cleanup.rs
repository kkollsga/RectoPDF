//! Fixture processes are observations; only direct Child handles authorize a kill.
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::Child;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct FixtureDir {
    dir: Option<recto_testpdf::TempDir>,
    failure: Arc<Mutex<Option<String>>>,
}

impl FixtureDir {
    pub fn new(dir: recto_testpdf::TempDir) -> Self {
        Self {
            dir: Some(dir),
            failure: Arc::new(Mutex::new(None)),
        }
    }
    pub fn own(&self, child: Child) -> OwnedChild {
        OwnedChild {
            child,
            failure: Arc::clone(&self.failure),
        }
    }
    pub fn retire(&mut self, observed: io::Result<()>) -> io::Result<()> {
        let failure = self.failure.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        let observed = observed.and_then(|()| failure.map_or(Ok(()), |error| Err(io::Error::other(error))));
        if observed.is_err()
            && let Some(dir) = self.dir.take()
        {
            std::mem::forget(dir);
        }
        observed
    }
}

impl std::ops::Deref for FixtureDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        self.dir.as_ref().expect("fixture retained only at teardown").path()
    }
}

pub struct OwnedChild {
    child: Child,
    failure: Arc<Mutex<Option<String>>>,
}

impl std::ops::Deref for OwnedChild {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.child
    }
}

impl std::ops::DerefMut for OwnedChild {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.child
    }
}

trait Anchor {
    fn exited(&mut self) -> io::Result<bool>;
    fn kill(&mut self) -> io::Result<()>;
}

impl Anchor for Child {
    fn exited(&mut self) -> io::Result<bool> {
        self.try_wait().map(|status| status.is_some())
    }
    fn kill(&mut self) -> io::Result<()> {
        Child::kill(self)
    }
}

fn retire_child(child: &mut impl Anchor, limit: Duration) -> io::Result<()> {
    // Any failed ownership check ends cleanup before a signal can be sent.
    if child.exited()? {
        return Ok(());
    }
    child.kill()?;
    wait(limit, || child.exited().map(|exited| !exited))
}

fn latch(failure: &Mutex<Option<String>>, error: io::Error) {
    let mut message = error.to_string();
    message.truncate(message.floor_char_boundary(1024));
    let mut failure = failure.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if failure.is_none() {
        *failure = Some(message);
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Err(error) = retire_child(&mut self.child, Duration::from_secs(1)) {
            eprintln!("owned installer child {} cleanup uncertain: {error}", self.child.id());
            latch(&self.failure, error);
        }
    }
}

#[allow(unsafe_code)] // Test-local libproc boundary, with fixed ABI/buffers below.
unsafe extern "C" {
    fn geteuid() -> u32;
    fn proc_listpids(kind: u32, uid: u32, buffer: *mut std::ffi::c_void, size: i32) -> i32;
    fn proc_pidpath(pid: i32, buffer: *mut std::ffi::c_void, size: u32) -> i32;
}

/// Own-UID scan with fixed caps. An unreadable process is unknown, never absence.
#[allow(unsafe_code)] // Only the bounded libproc observations below.
fn present(root: &Path) -> io::Result<bool> {
    let mut pids = [0i32; 8192];
    // SAFETY: a live fixed buffer with its exact byte count; PROC_UID_ONLY is 4.
    let bytes = unsafe { proc_listpids(4, geteuid(), pids.as_mut_ptr().cast(), size_of_val(&pids) as i32) };
    if bytes <= 0 || bytes as usize >= size_of_val(&pids) || !(bytes as usize).is_multiple_of(size_of::<i32>()) {
        return Err(io::Error::other("fixture process scan failed or reached its cap"));
    }
    let mut live = false;
    for pid in pids[..bytes as usize / size_of::<i32>()].iter().copied().filter(|pid| *pid > 1) {
        let mut path = [0u8; 4096];
        // SAFETY: a live fixed buffer and its size; this only observes a process.
        let count = unsafe { proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
        if count <= 0 {
            let error = io::Error::last_os_error();
            if process_vanished(&error) {
                continue;
            }
            return Err(error);
        }
        if count as usize >= path.len() {
            return Err(io::Error::other("fixture process path reached its cap"));
        }
        let end = path.iter().position(|byte| *byte == 0).unwrap_or(count as usize);
        let path = Path::new(std::ffi::OsStr::from_bytes(&path[..end]));
        if !path.is_absolute() {
            return Err(io::Error::other("fixture process path was not absolute"));
        }
        live |= path.starts_with(root);
    }
    Ok(live)
}

fn process_vanished(error: &io::Error) -> bool {
    error.raw_os_error() == Some(3) // Darwin ESRCH; ENOENT can be a live unlinked image.
}

pub fn wait_for_exit(root: &Path, limit: Duration) -> io::Result<()> {
    wait(limit, || present(root))
}

fn wait(limit: Duration, mut probe: impl FnMut() -> io::Result<bool>) -> io::Result<()> {
    let deadline = Instant::now() + limit;
    loop {
        if !probe()? {
            return Ok(());
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "fixture processes remain"));
        }
        std::thread::sleep(left.min(Duration::from_millis(10))); // ratchet: poll
    }
}

#[test]
/// Failure: unknown observations or a live fixture at the deadline permit deletion.
fn only_observed_absence_allows_fixture_removal() {
    assert!(process_vanished(&io::Error::from_raw_os_error(3)));
    assert!(!process_vanished(&io::Error::from_raw_os_error(2)), "a live unlinked image can return ENOENT");
    assert!(wait(Duration::ZERO, || Ok(false)).is_ok());
    assert!(wait(Duration::ZERO, || Ok(true)).is_err());
    assert!(wait(Duration::ZERO, || Err(io::Error::other("unknown"))).is_err());
    for state in ["absent", "live", "unknown"] {
        let dir = recto_testpdf::TempDir::new("installer-cleanup-policy-");
        let path = dir.path().to_path_buf();
        let mut held = FixtureDir::new(dir);
        let observed = wait(Duration::ZERO, || match state {
            "absent" => Ok(false),
            "live" => Ok(true),
            _ => Err(io::Error::other("unknown")),
        });
        assert_eq!(held.retire(observed).is_ok(), state == "absent");
        drop(held);
        assert_eq!(path.exists(), state != "absent", "{state}");
        if path.exists() {
            std::fs::remove_dir_all(path).unwrap();
        }
    }
}

#[test]
/// Failure: losing a Child anchor still authorizes a kill, or bounded reap uncertainty permits fixture deletion.
fn lost_child_anchor_never_signals_and_retains_the_fixture() {
    struct Fake {
        error: Option<i32>,
        kills: u32,
    }
    impl Anchor for Fake {
        fn exited(&mut self) -> io::Result<bool> {
            self.error.map_or(Ok(false), |code| Err(io::Error::from_raw_os_error(code)))
        }
        fn kill(&mut self) -> io::Result<()> {
            self.kills += 1;
            Ok(())
        }
    }
    let retained = |error: io::Error| {
        let dir = recto_testpdf::TempDir::new("installer-lost-anchor-");
        let path = dir.path().to_path_buf();
        let mut fixture = FixtureDir::new(dir);
        latch(&fixture.failure, error);
        assert!(fixture.retire(Ok(())).is_err(), "later observed absence cannot erase uncertainty");
        drop(fixture);
        assert!(path.exists(), "the uncertain fixture is retained");
        std::fs::remove_dir_all(path).unwrap();
    };
    // Darwin ECHILD=10; another failed ownership observation is equally uncertain.
    for code in [10, 1] {
        let mut child = Fake { error: Some(code), kills: 0 };
        let error = retire_child(&mut child, Duration::ZERO).unwrap_err();
        assert_eq!(child.kills, 0, "ownership error {code} must not signal");
        retained(error);
    }
    let mut child = Fake { error: None, kills: 0 };
    let error = retire_child(&mut child, Duration::ZERO).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert_eq!(child.kills, 1);
    retained(error);
}
