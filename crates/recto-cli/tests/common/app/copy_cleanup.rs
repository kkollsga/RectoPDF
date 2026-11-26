//! Cancels only private fixture program leases. Observed PIDs are never authority to signal.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

#[cfg(unix)]
use super::bed_folder;

/// No copies or filesystem changes when none of these fixture programs is running.
pub fn end(root: &Path, programs: &[PathBuf], wait: Duration) -> Result<(), String> {
    for program in programs {
        let pids = recto_platform::local::observe_missing_program_processes(program);
        if pids.is_empty() {
            continue;
        }
        if pids.len() > 256 {
            return Err(format!("too many observed fixture processes for {}", program.display()));
        }
        cancel(root, program, &pids, wait)?;
    }
    Ok(())
}

#[cfg(unix)]
fn owned_program(root: &Path, program: &Path) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("canonicalize fixture root {}: {error}", root.display()))?;
    let fixture = root
        .ancestors()
        .find(|path| {
            path.file_name().and_then(|name| name.to_str()).is_some_and(|name| {
                bed_folder::parse_own(name) == Some(std::process::id())
                    || bed_folder::parse(name).is_some_and(|made| {
                        made.pid == std::process::id()
                            && made.tree.as_deref()
                                == Some(&bed_folder::tree_of(
                                    &Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap(),
                                ))
                    })
            })
        })
        .ok_or("not a fixture owned by this test process")?;
    if !program.is_absolute()
        || !program.starts_with(&root)
        || program
            .canonicalize()
            .map_err(|error| format!("canonicalize fixture program {}: {error}", program.display()))?
            != program
    {
        return Err("fixture program must be a canonical path inside its owned root".into());
    }
    for path in program.ancestors().take_while(|path| path.starts_with(fixture)) {
        let meta = std::fs::symlink_metadata(path).map_err(|error| format!("inspect fixture path {}: {error}", path.display()))?;
        if meta.file_type().is_symlink() || meta.uid() != recto_platform::local::current_uid() || meta.mode() & 0o022 != 0 {
            return Err(format!("fixture path is not private to its owner: {}", path.display()));
        }
    }
    Ok(fixture.to_owned())
}

#[cfg(unix)]
fn same(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    a.dev() == b.dev() && a.ino() == b.ino() && a.len() == b.len() && a.mtime() == b.mtime() && a.mtime_nsec() == b.mtime_nsec()
}

/// A distinct inode keeps the bytes/mode while unlinking the vnode watched by the quiet app.
#[cfg(unix)]
pub struct Lease {
    program: PathBuf,
    backup: PathBuf,
}

#[cfg(unix)]
impl Lease {
    pub fn remove(root: &Path, program: &Path) -> Result<Self, String> {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let fixture = owned_program(root, program)?;
        let mut source = std::fs::File::open(program).map_err(|error| format!("open fixture program {}: {error}", program.display()))?;
        let before = source
            .metadata()
            .map_err(|error| format!("inspect open fixture program {}: {error}", program.display()))?;
        if !before.is_file() || before.mode() & 0o6000 != 0 {
            return Err("fixture program must be a regular executable without elevated mode bits".into());
        }
        let fixture_meta = std::fs::metadata(&fixture).map_err(|error| format!("inspect fixture backup root {}: {error}", fixture.display()))?;
        if fixture_meta.dev() != before.dev() {
            return Err("fixture backup root and program must be on the same filesystem".into());
        }
        let backup = fixture.join(format!(".recto-cleanup-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let mut saved = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&backup)
            .map_err(|error| format!("create fixture backup {}: {error}", backup.display()))?;
        let prepared = (|| -> Result<(), String> {
            std::io::copy(&mut source, &mut saved)
                .map_err(|error| format!("copy fixture program {} to backup {}: {error}", program.display(), backup.display()))?;
            saved
                .set_permissions(std::fs::Permissions::from_mode(before.mode() & 0o777))
                .map_err(|error| format!("set fixture backup permissions {}: {error}", backup.display()))?;
            let source_after = source
                .metadata()
                .map_err(|error| format!("recheck open fixture program {}: {error}", program.display()))?;
            let path_after = std::fs::symlink_metadata(program).map_err(|error| format!("recheck fixture program pathname {}: {error}", program.display()))?;
            if !same(&before, &source_after) || !same(&before, &path_after) {
                return Err(format!("fixture program {} changed while its backup was made", program.display()));
            }
            std::fs::remove_file(program).map_err(|error| format!("unlink fixture program {}: {error}", program.display()))
        })();
        if let Err(error) = prepared {
            return match std::fs::remove_file(&backup) {
                Ok(()) => Err(error),
                Err(cleanup) => Err(format!(
                    "{error}; remove unused fixture backup {} failed (retained): {cleanup}",
                    backup.display()
                )),
            };
        }
        Ok(Self {
            program: program.to_owned(),
            backup,
        })
    }

    /// Publishing never overwrites a replacement created while the lease was absent.
    pub fn restore(&self) -> Result<(), String> {
        std::fs::hard_link(&self.backup, &self.program).map_err(|error| {
            format!(
                "restore fixture program {} with nonoverwriting hard link; backup retained at {}: {error}",
                self.program.display(),
                self.backup.display()
            )
        })?;
        std::fs::remove_file(&self.backup).map_err(|error| format!("remove restored fixture backup {} failed (retained): {error}", self.backup.display()))
    }
}

/// What a quiet run is given to be up (`qa_launch::READY_CAP`, 180 s) and to have armed its end-watch, which it does before its ready line, plus this to exit once the watch fires.
pub const EXIT_GRACE: Duration = Duration::from_secs(10);

/// How long cleanup keeps a copy's program unlinked while it waits for the runs to exit. A run still starting when the lease is taken arms its watch
/// within `READY_CAP`, finds the program gone and exits; a shorter wait gives up on a run that is about to end (a 5 s wait did, under load 23).
pub const EXIT_CAP: Duration = Duration::from_secs(super::qa_launch::READY_CAP.as_secs() + EXIT_GRACE.as_secs());

/// Takes the program away (the lease), waits for every run of it to exit, and puts it back. The lease is kept until the exit, not given back at the
/// first sign of slowness: a run that arms its watch after the program is back never sees it go and lives on. Each wait is one `kevent` on the
/// processes' own exit. No PID is signalled.
///
/// Known limit: only the runs in `initial`, observed before the lease, are waited for. While the program is unlinked the system tells no path for a running
/// copy (`proc_pidpath` is empty), so no look during the lease can find a copy; one launched after `end`'s observation is not waited for.
#[cfg(unix)]
pub fn cancel(root: &Path, program: &Path, initial: &[u32], wait: Duration) -> Result<(), String> {
    cancel_by(root, program, initial, wait, start_time)
}

/// When a process started, as the system tells it (seconds and microseconds), or `None` once it is gone. A pid's number can be given to another process after
/// the copy exited; a different start time says it was, so it is not waited for as the copy. (Its path can't say: it is empty for a running copy whose program is unlinked.)
#[cfg(unix)]
#[allow(unsafe_code)] // One read-only `proc_pidinfo`, as `recto-platform` makes; a test helper, so not worth a new public function there.
pub fn start_time(pid: u32) -> Option<(i64, i64)> {
    let pid = libc::c_int::try_from(pid).ok()?;
    // SAFETY: observation only; `info` is a zeroed plain struct, filled by the call with exactly its size.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `info` is exactly `size` bytes of a plain struct the call fills, and the pid is only a number; nothing else is read or written.
    let got = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    (got == size).then_some((info.pbi_start_tvsec as i64, info.pbi_start_tvusec as i64))
}

/// [`cancel`], with how a pid's start time is told, so a reused pid can be made in a test.
#[cfg(unix)]
pub fn cancel_by(root: &Path, program: &Path, initial: &[u32], wait: Duration, started: impl Fn(u32) -> Option<(i64, i64)>) -> Result<(), String> {
    let lease = Lease::remove(root, program)?;
    let deadline = Instant::now() + wait;
    let mut alive: Vec<(u32, (i64, i64))> = Vec::new();
    for pid in initial {
        if !alive.iter().any(|(known, _)| known == pid)
            && let Some(at) = started(*pid)
        {
            alive.push((*pid, at));
        }
    }
    let ended = loop {
        alive.retain(|(pid, at)| started(*pid) == Some(*at));
        if alive.is_empty() {
            break true;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break false;
        }
        let waited = Instant::now();
        let pids: Vec<u32> = alive.iter().map(|(pid, _)| *pid).collect();
        recto_platform::local::wait_until_gone(&pids, &[], left);
        if waited.elapsed() < Duration::from_millis(20) {
            // An exit already seen but not yet reaped by its parent answers at once: don't spin on it.
            std::thread::sleep(Duration::from_millis(20)); // ratchet: poll
        }
    };
    let restored = lease.restore();
    if !ended {
        return Err(format!(
            "fixture processes did not exit within {wait:?}: {} (still running: {:?}); no PID was signalled; restoration: {restored:?}",
            program.display(),
            alive.iter().map(|(pid, _)| *pid).collect::<Vec<_>>()
        ));
    }
    restored
}

#[cfg(not(unix))]
fn cancel(_root: &Path, _program: &Path, _initial: &[u32], _wait: Duration) -> Result<(), String> {
    Err("fixture lease cancellation is unavailable here; no PID was signalled".into())
}
