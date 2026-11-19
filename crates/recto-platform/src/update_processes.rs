//! Bounded update-process observation. A PID here never authorizes a signal.
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

const MAX_PIDS: usize = 8192;
const MAX_PATH: usize = 4096;

trait Scan {
    fn list(&mut self, buffer: &mut [i32]) -> io::Result<usize>;
    fn path(&mut self, pid: i32, buffer: &mut [u8]) -> io::Result<usize>;
}

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

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        list_bytes: usize,
        path: &'static [u8],
        error: Option<i32>,
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
        fake.error = Some(libc::ESRCH);
        assert!(!observe(wanted, &mut fake).unwrap());
    }

    /// Failure: the native ABI or path handling silently misses this process.
    #[test]
    fn native_observation_finds_the_current_program() {
        assert!(update_app_is_running(&std::env::current_exe().unwrap()).unwrap());
        assert!(update_app_is_running(Path::new("/no/such/update/program")).is_err());
    }
}
