//! Fixture processes are observations; only direct Child handles authorize a kill.
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::Child;
use std::time::{Duration, Instant};

pub struct FixtureDir(Option<recto_testpdf::TempDir>);

impl FixtureDir {
    pub fn new(dir: recto_testpdf::TempDir) -> Self {
        Self(Some(dir))
    }
    pub fn retire(&mut self, observed: io::Result<()>) -> io::Result<()> {
        if observed.is_err() {
            if let Some(dir) = self.0.take() {
                std::mem::forget(dir);
            }
        }
        observed
    }
}

impl std::ops::Deref for FixtureDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        self.0.as_ref().expect("fixture retained only at teardown").path()
    }
}

pub struct OwnedChild(pub Child);

impl std::ops::Deref for OwnedChild {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}

impl std::ops::DerefMut for OwnedChild {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(Some(_))) {
            return;
        }
        // Still our unreaped direct child, never a PID learned from enumeration.
        let _ = self.0.kill();
        let _ = self.0.wait();
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
    if bytes <= 0 || bytes as usize >= size_of_val(&pids) || bytes as usize % size_of::<i32>() != 0 {
        return Err(io::Error::other("fixture process scan failed or reached its cap"));
    }
    let mut live = false;
    for pid in pids[..bytes as usize / size_of::<i32>()].iter().copied().filter(|pid| *pid > 1) {
        let mut path = [0u8; 4096];
        // SAFETY: a live fixed buffer and its size; this only observes a process.
        let count = unsafe { proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
        if count <= 0 {
            let error = io::Error::last_os_error();
            if matches!(error.raw_os_error(), Some(2) | Some(3)) {
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
