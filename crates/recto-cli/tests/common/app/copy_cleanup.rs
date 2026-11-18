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
fn owned_program(root: &Path, program: &Path) -> Result<(), String> {
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
    Ok(())
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
        owned_program(root, program)?;
        let mut source = std::fs::File::open(program).map_err(|error| format!("open fixture program {}: {error}", program.display()))?;
        let before = source
            .metadata()
            .map_err(|error| format!("inspect open fixture program {}: {error}", program.display()))?;
        if !before.is_file() || before.mode() & 0o6000 != 0 {
            return Err("fixture program must be a regular executable without elevated mode bits".into());
        }
        let backup = program.with_file_name(format!(".recto-cleanup-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
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

#[cfg(unix)]
pub fn cancel(root: &Path, program: &Path, initial: &[u32], wait: Duration) -> Result<(), String> {
    let lease = Lease::remove(root, program)?;
    let began = Instant::now();
    let ended = loop {
        let observed_alive = initial.iter().any(|pid| recto_platform::local::process_counters(*pid).is_some());
        let fresh = recto_platform::local::observe_missing_program_processes(program);
        if !observed_alive && fresh.is_empty() {
            break true;
        }
        let Some(left) = wait.checked_sub(began.elapsed()) else { break false };
        std::thread::sleep(left.min(Duration::from_millis(20))); // ratchet: poll
    };
    let restored = lease.restore();
    if !ended {
        return Err(format!(
            "fixture processes did not exit within {wait:?}: {}; no PID was signalled; restoration: {restored:?}",
            program.display()
        ));
    }
    restored
}

#[cfg(not(unix))]
fn cancel(_root: &Path, _program: &Path, _initial: &[u32], _wait: Duration) -> Result<(), String> {
    Err("fixture lease cancellation is unavailable here; no PID was signalled".into())
}
