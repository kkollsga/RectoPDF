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
    retire_child_with(child, limit, Instant::now, std::thread::sleep)
}

fn retire_child_with(child: &mut impl Anchor, limit: Duration, now: impl FnMut() -> Instant, pause: impl FnMut(Duration)) -> io::Result<()> {
    // Any failed ownership check ends cleanup before a signal can be sent.
    if child.exited()? {
        return Ok(());
    }
    child.kill()?;
    wait_with(limit, || child.exited().map(|exited| !exited), now, pause, Unknown::Refuse)
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

const MAX_PIDS: usize = 8192;
const MAX_PATH: usize = 4096;
/// The start names of the programs a fixture runs: its two copies of the helper (`bundle_at`). A program whose file
/// is gone and that was started under one of these may be the fixture's own, so it stays unknown.
pub const FIXTURE_PROGRAMS: [&[u8]; 2] = [b"RectoPDF", b"recto-updater"];

trait Scan {
    fn list(&mut self, buffer: &mut [i32]) -> io::Result<usize>;
    fn path(&mut self, pid: i32, buffer: &mut [u8]) -> io::Result<usize>;
    /// The name the process was started under (`pbi_comm`, which only exec writes), cut by the kernel at 15 bytes.
    fn start_name(&mut self, pid: i32) -> io::Result<Vec<u8>>;
}

struct Mac;

#[allow(unsafe_code)] // Test-local libproc boundary, with fixed ABI/buffers.
impl Scan for Mac {
    fn list(&mut self, buffer: &mut [i32]) -> io::Result<usize> {
        // SAFETY: PROC_UID_ONLY is 4; a live fixed buffer with its exact byte count.
        let bytes = unsafe { libc::proc_listpids(4, libc::geteuid(), buffer.as_mut_ptr().cast(), size_of_val(buffer) as i32) };
        usize::try_from(bytes).map_err(|_| io::Error::last_os_error())
    }
    fn path(&mut self, pid: i32, buffer: &mut [u8]) -> io::Result<usize> {
        // SAFETY: a live fixed buffer and its size; this only observes a process.
        let count = unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
        if count <= 0 { Err(io::Error::last_os_error()) } else { Ok(count as usize) }
    }
    fn start_name(&mut self, pid: i32) -> io::Result<Vec<u8>> {
        // SAFETY: observation only; a zeroed plain struct and its exact size.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = size_of::<libc::proc_bsdinfo>() as i32;
        let got = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
        if got != size {
            return Err(if got <= 0 {
                io::Error::last_os_error()
            } else {
                io::Error::other("short process info")
            });
        }
        Ok(info.pbi_comm.iter().map(|byte| *byte as u8).take_while(|byte| *byte != 0).collect())
    }
}

/// Own-UID scan with fixed caps. An unreadable process of the fixture's is unknown, never absence.
fn present(root: &Path) -> io::Result<bool> {
    observe(root, &FIXTURE_PROGRAMS, &mut Mac)
}

fn observe(root: &Path, programs: &[&[u8]], scan: &mut impl Scan) -> io::Result<bool> {
    let mut pids = [0i32; MAX_PIDS];
    let bytes = scan.list(&mut pids)?;
    if bytes == 0 || bytes >= size_of_val(&pids) || !bytes.is_multiple_of(size_of::<i32>()) {
        return Err(io::Error::other("fixture process scan failed or reached its cap"));
    }
    let mut live = false;
    let mut path = [0u8; MAX_PATH];
    for pid in pids[..bytes / size_of::<i32>()].iter().copied().filter(|pid| *pid > 1) {
        path.fill(0);
        let count = match scan.path(pid, &mut path) {
            Ok(count) => count,
            Err(error) if process_vanished(&error) => continue,
            // ENOENT is a live program whose file was deleted or replaced (another app updated under a long-running
            // process of this user). The product's scan (`update_processes::started_as`) skips it when the name it was
            // started under can't be the app's: the same rule, with the fixture's program names. Its own, or one whose
            // name can't be read, stays unknown.
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => match scan.start_name(pid) {
                Err(error) if process_vanished(&error) => continue,
                Ok(name) if !name.is_empty() && !programs.iter().any(|program| program.starts_with(&name)) => continue,
                _ => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!("process path query for PID {pid} while observing {}: {error}", root.display()),
                    ));
                }
            },
            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!("process path query for PID {pid} while observing {}: {error}", root.display()),
                ));
            }
        };
        if count >= path.len() {
            return Err(io::Error::other("fixture process path reached its cap"));
        }
        let end = path.iter().position(|byte| *byte == 0).unwrap_or(count);
        let path = Path::new(std::ffi::OsStr::from_bytes(&path[..end]));
        if !path.is_absolute() {
            return Err(io::Error::other("fixture process path was not absolute"));
        }
        live |= path.starts_with(root);
    }
    Ok(live)
}

fn process_vanished(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::ESRCH)
}

pub fn wait_for_exit(root: &Path, limit: Duration) -> io::Result<()> {
    wait(limit, || present(root))
}

/// Waits until the probe says absent (`Ok(false)`). An unknown answer (an `Err`) is "not yet known", as in the
/// product's `known`: it is polled past, and returned if the limit passes.
pub fn wait(limit: Duration, probe: impl FnMut() -> io::Result<bool>) -> io::Result<()> {
    wait_with(limit, probe, Instant::now, std::thread::sleep, Unknown::Retry)
}

#[derive(Clone, Copy)]
enum Unknown {
    Retry,
    Refuse,
}

fn wait_with(
    limit: Duration,
    mut probe: impl FnMut() -> io::Result<bool>,
    mut now: impl FnMut() -> Instant,
    mut pause: impl FnMut(Duration),
    unknown_policy: Unknown,
) -> io::Result<()> {
    let deadline = now() + limit;
    loop {
        let unknown = match probe() {
            Ok(false) => return Ok(()),
            Ok(true) => None,
            Err(error) => match unknown_policy {
                Unknown::Retry => Some(error),
                Unknown::Refuse => return Err(error),
            },
        };
        let left = deadline.saturating_duration_since(now());
        if left.is_zero() {
            return Err(unknown.unwrap_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "fixture processes remain")));
        }
        pause(left.min(Duration::from_millis(10)));
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

/// Failure: a single unknown OS observation rejects otherwise-proved cleanup,
/// or persistent uncertainty is accepted as exit after patience runs out.
#[test]
fn uncertain_observation_retries_without_weakening_exit_proof() {
    let began = Instant::now();
    let clock = std::cell::Cell::new(began);
    let limit = Duration::from_nanos(2);
    let mut samples = [Err(io::Error::from_raw_os_error(2)), Ok(false)].into_iter();
    assert!(
        wait_with(
            limit,
            || samples.next().unwrap(),
            || clock.get(),
            |step| clock.set(clock.get() + step),
            Unknown::Retry
        )
        .is_ok()
    );
    clock.set(began);
    let calls = std::cell::Cell::new(0);
    let result = wait_with(
        limit,
        || {
            calls.set(calls.get() + 1);
            Err(io::Error::from_raw_os_error(2))
        },
        || clock.get(),
        |step| clock.set(clock.get() + step),
        Unknown::Retry,
    );
    assert!(result.is_err());
    assert_eq!(clock.get(), began + limit);
    assert_eq!(calls.get(), 2);
}

/// Failure: an error after killing an owned child is retried and erased by a
/// later reap, so its fixture is removed despite lost ownership evidence.
#[test]
fn a_post_kill_ownership_error_is_latched_without_another_probe() {
    struct Fake {
        calls: usize,
        kills: usize,
    }
    impl Anchor for Fake {
        fn exited(&mut self) -> io::Result<bool> {
            self.calls += 1;
            match self.calls {
                1 => Ok(false),
                2 => Err(io::Error::from_raw_os_error(10)),
                _ => Ok(true),
            }
        }
        fn kill(&mut self) -> io::Result<()> {
            self.kills += 1;
            Ok(())
        }
    }
    let began = Instant::now();
    let clock = std::cell::Cell::new(began);
    let mut child = Fake { calls: 0, kills: 0 };
    let result = retire_child_with(&mut child, Duration::from_nanos(2), || clock.get(), |step| clock.set(clock.get() + step));
    assert!(result.is_err(), "post-kill ownership error must not become a successful reap");
    assert_eq!((child.calls, child.kills), (2, 1));
    assert_eq!(clock.get(), began, "ownership error returns without a retry");
    let dir = recto_testpdf::TempDir::new("installer-post-kill-anchor-");
    let path = dir.path().to_path_buf();
    let mut fixture = FixtureDir::new(dir);
    latch(&fixture.failure, result.unwrap_err());
    assert!(fixture.retire(Ok(())).is_err());
    drop(fixture);
    assert!(path.exists());
    std::fs::remove_dir_all(path).unwrap();
}

/// Failure: a long-lived program of the person's own whose file was replaced (ENOENT on its path, as the ChatGPT
/// crashpad handler was on 2026-11-28) fails every installer test's teardown, or the fixture's own program that
/// lost its file is taken for another and the fixture is deleted under it.
#[test]
fn a_foreign_deleted_program_is_ignored_but_a_fixtures_own_is_not() {
    struct Fake {
        path_error: Option<i32>,
        name: Option<&'static [u8]>,
        path: &'static [u8],
    }
    impl Scan for Fake {
        fn list(&mut self, buffer: &mut [i32]) -> io::Result<usize> {
            buffer[0] = 42;
            Ok(4)
        }
        fn path(&mut self, _: i32, buffer: &mut [u8]) -> io::Result<usize> {
            if let Some(code) = self.path_error {
                return Err(io::Error::from_raw_os_error(code));
            }
            buffer[..self.path.len()].copy_from_slice(self.path);
            Ok(self.path.len())
        }
        fn start_name(&mut self, _: i32) -> io::Result<Vec<u8>> {
            self.name.map(<[u8]>::to_vec).ok_or_else(|| io::Error::from_raw_os_error(libc::EPERM))
        }
    }
    let root = Path::new("/private/tmp/fixture");
    let mut fake = Fake {
        path_error: Some(libc::ENOENT),
        name: Some(b"browser_crashpad"),
        path: b"",
    };
    assert!(
        !observe(root, &FIXTURE_PROGRAMS, &mut fake).unwrap(),
        "another program, deleted: not the fixture's"
    );
    for own in [&b"RectoPDF"[..], b"recto-updater"] {
        fake.name = Some(own);
        assert!(
            observe(root, &FIXTURE_PROGRAMS, &mut fake).is_err(),
            "the fixture's own program that lost its file stays unknown"
        );
    }
    fake.name = None;
    assert!(observe(root, &FIXTURE_PROGRAMS, &mut fake).is_err(), "an unreadable start name stays unknown");
    fake.name = Some(b"");
    assert!(observe(root, &FIXTURE_PROGRAMS, &mut fake).is_err(), "an empty start name stays unknown");
    fake.path_error = None;
    fake.path = b"/private/tmp/fixture/Contents/MacOS/RectoPDF\0";
    assert!(observe(root, &FIXTURE_PROGRAMS, &mut fake).unwrap(), "a live program at the root is seen");
    fake.path = b"/usr/bin/sleep\0";
    assert!(!observe(root, &FIXTURE_PROGRAMS, &mut fake).unwrap());
}
