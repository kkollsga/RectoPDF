//! Bounded update-process observation. A PID here never authorizes a signal.
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

const MAX_PIDS: usize = 8192;
const MAX_PATH: usize = 4096;

trait Scan {
    fn list(&mut self, buffer: &mut [i32]) -> io::Result<usize>;
    fn path(&mut self, pid: i32, buffer: &mut [u8]) -> io::Result<usize>;
    /// The name the process was started under, written into `buffer`: at most `COMM_LEN` bytes, the kernel's cut.
    fn name(&mut self, pid: i32, buffer: &mut [u8; COMM_LEN]) -> io::Result<usize>;
}

/// The longest start name the kernel keeps: `pbi_comm` holds `MAXCOMLEN` bytes and xnu ends it with a NUL at the
/// last one, so the usable length is `MAXCOMLEN - 1` (`universalaccessd` reads `universalaccess`).
const COMM_LEN: usize = libc::MAXCOMLEN - 1;

struct Mac;

impl Scan for Mac {
    fn list(&mut self, buffer: &mut [i32]) -> io::Result<usize> {
        unsafe extern "C" {
            fn proc_listpids(kind: u32, uid: u32, buffer: *mut std::ffi::c_void, size: i32) -> i32;
        }
        // SAFETY: PROC_UID_ONLY=4, this user's ID and a fixed live buffer with its exact size.
        let bytes = unsafe { proc_listpids(4, libc::geteuid(), buffer.as_mut_ptr().cast(), size_of_val(buffer) as i32) };
        usize::try_from(bytes).map_err(|_| io::Error::last_os_error())
    }

    fn path(&mut self, pid: i32, buffer: &mut [u8]) -> io::Result<usize> {
        // SAFETY: observation only, with a live fixed buffer and its exact size.
        let bytes = unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
        if bytes <= 0 { Err(io::Error::last_os_error()) } else { Ok(bytes as usize) }
    }

    fn name(&mut self, pid: i32, buffer: &mut [u8; COMM_LEN]) -> io::Result<usize> {
        // SAFETY: observation only; `info` is a zeroed plain struct and its exact size is passed.
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
        let mut count = 0;
        for (slot, byte) in buffer.iter_mut().zip(info.pbi_comm.iter().map(|byte| *byte as u8)) {
            if byte == 0 {
                break;
            }
            *slot = byte;
            count += 1;
        }
        Ok(count)
    }
}

/// Whether this user's processes include the exact update executable.
/// Fixed caps; enumeration or path errors are uncertainty, never absence.
///
/// # Errors
/// The executable cannot be resolved, the OS query fails or reaches its cap,
/// or an observed process cannot be classified safely.
pub fn update_app_is_running(executable: &Path) -> io::Result<bool> {
    let wanted = executable.canonicalize()?;
    observe(&wanted, &mut Mac)
}

fn observe(wanted: &Path, scan: &mut impl Scan) -> io::Result<bool> {
    let mut pids = [0i32; MAX_PIDS];
    let bytes = scan.list(&mut pids)?;
    if bytes == 0 || bytes >= size_of_val(&pids) || !bytes.is_multiple_of(size_of::<i32>()) {
        return Err(io::Error::other("update process list failed or reached its cap"));
    }
    let mut path = [0u8; MAX_PATH];
    let mut live = false;
    for pid in pids[..bytes / size_of::<i32>()].iter().copied().filter(|pid| *pid > 1) {
        path.fill(0);
        let count = match scan.path(pid, &mut path) {
            Ok(count) => count,
            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => continue,
            // A live program whose file was deleted answers ENOENT. Owner, 2026-11-25 (`docs/OWNER-RULES.md`,
            // "Decisions of 2026-11-25"): ignore it when the name it was started under shows it can't be the
            // app. The same name, or a name that can't be read, stays unknown, so the app is never replaced
            // while it might be open.
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => match started_as(wanted, scan, pid) {
                Start::Other => continue,
                Start::Unknown(name) => return Err(deleted_executable(pid, &name, &error)),
            },
            Err(error) => return Err(error),
        };
        if count == 0 || count >= path.len() {
            return Err(io::Error::other("update process path failed or reached its cap"));
        }
        let end = path[..count].iter().position(|byte| *byte == 0).unwrap_or(count);
        let found = Path::new(std::ffi::OsStr::from_bytes(&path[..end]));
        if !found.is_absolute() {
            return Err(io::Error::other("update process path is not absolute"));
        }
        live |= found == wanted;
    }
    Ok(live)
}

/// What the start name of a live program whose file was deleted shows.
enum Start {
    /// It can't be the wanted program.
    Other,
    /// It could be: the name read (or why it couldn't be), for the unknown reported.
    Unknown(String),
}

/// The unknown for a live program whose file was deleted and that could be the app: says which process, by the same
/// name the decision was made on, and keeps `ErrorKind::NotFound`.
fn deleted_executable(pid: i32, name: &str, cause: &io::Error) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("pid {pid} ({name}) has a deleted executable: {cause}"))
}

/// Reads the start name once. It is `Other` only when the name shows the process can't be the wanted program. The kernel
/// cuts the name at `COMM_LEN` bytes: a shorter name must differ from the wanted file name in full; a name of exactly
/// `COMM_LEN` may be a cut, so it is skipped only when it is not a prefix of the wanted name. A name that can't be read
/// stays unknown, except that a process that vanished (`ESRCH`) is skipped as `path` skips it.
/// Known limit: the start name is the last part of the path the program was started through, so the app's own executable
/// run through a link of another name, and then deleted while running, would be skipped. The app as launched normally
/// runs under its own name, so a running copy of it stays unknown.
fn started_as(wanted: &Path, scan: &mut impl Scan, pid: i32) -> Start {
    let mut name = [0u8; COMM_LEN];
    let count = match scan.name(pid, &mut name) {
        Ok(count) => count,
        Err(error) if error.raw_os_error() == Some(libc::ESRCH) => return Start::Other,
        Err(_) => return Start::Unknown("unreadable name".into()),
    };
    let Some(read) = name.get(..count).filter(|read| !read.is_empty()) else {
        return Start::Unknown("unreadable name".into());
    };
    let other = wanted.file_name().is_some_and(|wanted| {
        let wanted = wanted.as_bytes();
        if count == COMM_LEN { !wanted.starts_with(read) } else { read != wanted }
    });
    if other {
        Start::Other
    } else {
        Start::Unknown(String::from_utf8_lossy(read).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        list_bytes: usize,
        path: &'static [u8],
        error: Option<i32>,
        started_as: Option<&'static [u8]>,
        name_error: Option<i32>,
    }
    impl Scan for Fake {
        fn list(&mut self, buffer: &mut [i32]) -> io::Result<usize> {
            buffer[0] = 42;
            Ok(self.list_bytes)
        }
        fn path(&mut self, _: i32, buffer: &mut [u8]) -> io::Result<usize> {
            if let Some(error) = self.error {
                return Err(io::Error::from_raw_os_error(error));
            }
            buffer[..self.path.len()].copy_from_slice(self.path);
            Ok(self.path.len())
        }
        fn name(&mut self, _: i32, buffer: &mut [u8; COMM_LEN]) -> io::Result<usize> {
            let name = self
                .started_as
                .ok_or_else(|| io::Error::from_raw_os_error(self.name_error.unwrap_or(libc::EPERM)))?;
            buffer[..name.len()].copy_from_slice(name);
            Ok(name.len())
        }
    }

    /// Failure: a failed/truncated process scan is mistaken for no writer,
    /// or a program's path is compared with a trailing NUL and missed.
    #[test]
    fn absence_requires_a_complete_scan_and_exact_paths() {
        let wanted = Path::new("/private/app/RectoPDF");
        let mut fake = Fake {
            list_bytes: 4,
            path: b"/private/app/RectoPDF\0",
            error: None,
            started_as: None,
            name_error: None,
        };
        assert!(observe(wanted, &mut fake).unwrap());
        fake.path = b"/private/other/RectoPDF\0";
        assert!(!observe(wanted, &mut fake).unwrap());
        for bytes in [0, 1, MAX_PIDS * size_of::<i32>()] {
            fake.list_bytes = bytes;
            assert!(observe(wanted, &mut fake).is_err());
        }
        fake.list_bytes = 4;
        for error in [libc::EPERM, libc::EIO, libc::ENOENT] {
            fake.error = Some(error);
            assert!(observe(wanted, &mut fake).is_err());
        }
        // ENOENT is a live program whose file was deleted: skipped only when its start name can't be the app's.
        fake.error = Some(libc::ENOENT);
        fake.started_as = Some(b"sleep");
        assert!(!observe(wanted, &mut fake).unwrap());
        fake.started_as = Some(b"RectoPDF");
        assert!(observe(wanted, &mut fake).is_err(), "the app's own name stays unknown");
        let error = observe(wanted, &mut fake).unwrap_err();
        assert!(error.to_string().contains("pid 42 (RectoPDF)"), "{error}");
        fake.started_as = None;
        assert!(observe(wanted, &mut fake).is_err(), "an unreadable name stays unknown");
        fake.error = Some(libc::ESRCH);
        assert!(!observe(wanted, &mut fake).unwrap());
    }

    /// Failure: the kernel keeps at most 15 bytes of a start name (`universalaccessd` reads `universalaccess`), so a
    /// running, deleted copy of an app with a longer executable name is skipped as "another program" and the installer
    /// replaces a running app. A cut name that is a prefix of the wanted name stays unknown.
    #[test]
    fn a_start_name_cut_by_the_kernel_is_compared_as_a_prefix() {
        let long = Path::new("/private/app/A-very-long-program-name");
        let mut fake = Fake {
            list_bytes: 4,
            path: b"",
            error: Some(libc::ENOENT),
            started_as: Some(b"A-very-long-pro"),
            name_error: None,
        };
        assert_eq!(fake.started_as.unwrap().len(), COMM_LEN);
        assert!(
            observe(long, &mut fake).is_err(),
            "a 15-byte cut that is a prefix of the wanted name is unknown"
        );
        fake.started_as = Some(b"Another-long-pr");
        assert!(!observe(long, &mut fake).unwrap(), "a 15-byte cut that is not a prefix is another program");
        let exactly = Path::new("/private/app/Exactly-15-byte");
        fake.started_as = Some(b"Exactly-15-byte");
        assert!(
            observe(exactly, &mut fake).is_err(),
            "a name of exactly the cap that is the whole wanted name is unknown"
        );
        let short = Path::new("/private/app/Short");
        fake.started_as = Some(b"Shor");
        assert!(!observe(short, &mut fake).unwrap(), "a short name below the cap is compared in full");
        fake.started_as = Some(b"Short");
        assert!(observe(short, &mut fake).is_err(), "the same short name is unknown");
        fake.started_as = Some(b"A-very-long-pro");
        assert!(!observe(short, &mut fake).unwrap(), "a 15-byte name can't be a prefix of a shorter wanted name");
        fake.started_as = None;
        fake.name_error = Some(libc::ESRCH);
        assert!(
            !observe(short, &mut fake).unwrap(),
            "a process that vanished between its path and its name is gone"
        );
        fake.name_error = Some(libc::EPERM);
        assert!(observe(short, &mut fake).is_err(), "any other name error is unknown");
    }

    struct Children(Vec<std::process::Child>);
    impl Drop for Children {
        fn drop(&mut self) {
            for child in &mut self.0 {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    /// A copy of `/bin/sleep` named `name`, started with its executable then deleted: a live process whose
    /// image is gone, which the OS answers a path query for with ENOENT.
    fn deleted_sleeper(name: &str) -> (recto_testpdf::TempDir, Children) {
        let dir = recto_testpdf::TempDir::new("update-processes-deleted-");
        let copy = dir.path().join(name);
        std::fs::copy("/bin/sleep", &copy).unwrap();
        // A plain copy of a system binary is killed at exec; an ad-hoc signature lets it run.
        let signed = std::process::Command::new("/usr/bin/codesign")
            .args(["-s", "-", "-f"])
            .arg(&copy)
            .output()
            .unwrap_or_else(|error| panic!("codesign could not be run: {error}"));
        assert!(
            signed.status.success(),
            "codesign failed ({}): {}",
            signed.status,
            String::from_utf8_lossy(&signed.stderr)
        );
        let mut children = Children(Vec::new());
        children.0.push(std::process::Command::new(&copy).arg("10").spawn().unwrap());
        std::fs::remove_file(&copy).unwrap();
        (dir, children)
    }

    /// Failure: an unrelated program of this user whose file was deleted (an updated tool still running) makes
    /// every observation of the update executable unknown, so an update can't go ahead (owner, 2026-11-25,
    /// `docs/OWNER-RULES.md` "Decisions of 2026-11-25"). A deleted program that could still be RectoPDF must
    /// stay unknown, so RectoPDF is never replaced while it might be open.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_deleted_program_started_under_another_name_is_skipped_but_one_named_like_the_app_is_not() {
        let (_other, _children) = deleted_sleeper("sleep");
        assert!(update_app_is_running(&std::env::current_exe().unwrap()).unwrap());

        let dir = recto_testpdf::TempDir::new("update-processes-wanted-");
        // A name of this run only: a deleted copy under the app's real name would make every other installer test, and
        // a real updater on this Mac, see an unknown while it lives. The rule under test is "the same name".
        let unique = format!("rpdf-{}", std::process::id());
        assert!(unique.len() < COMM_LEN);
        let wanted = dir.path().join(&unique);
        std::fs::copy("/bin/sleep", &wanted).unwrap();
        let (_same_name, _more) = deleted_sleeper(&unique);
        let error = update_app_is_running(&wanted).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound, "{error}");
        assert!(
            error.to_string().contains("has a deleted executable"),
            "the unknown says which process: {error}"
        );
    }

    /// Failure: the native ABI or path handling silently misses this process.
    #[test]
    fn native_observation_finds_the_current_program() {
        assert!(update_app_is_running(&std::env::current_exe().unwrap()).unwrap());
        assert!(update_app_is_running(Path::new("/no/such/update/program")).is_err());
    }
}
