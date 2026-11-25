//! Bounded update-process observation. A PID here never authorizes a signal.
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

const MAX_PIDS: usize = 8192;
const MAX_PATH: usize = 4096;

trait Scan {
    fn list(&mut self, buffer: &mut [i32]) -> io::Result<usize>;
    fn path(&mut self, pid: i32, buffer: &mut [u8]) -> io::Result<usize>;
    /// The name the process was started under, written into `buffer`: its length, and the length at which the kernel
    /// cuts the name it was read from (a name of exactly that length may be a cut). `pbi_name` is not that name by
    /// itself, because the process can rewrite it: it counts only where it agrees with `pbi_comm` (`pick_name`).
    fn name(&mut self, pid: i32, buffer: &mut [u8; NAME_LEN]) -> io::Result<Name>;
}

/// The longest start name `pbi_name` keeps when it agrees with `pbi_comm`: it holds `2 * MAXCOMLEN` bytes and xnu ends it with a NUL at the last one.
const NAME_LEN: usize = 2 * libc::MAXCOMLEN - 1;
/// The longest start name `pbi_comm` keeps (`universalaccessd` reads `universalaccess`). Only exec writes it, so it is
/// the start name; it is used whole when `pbi_name` is empty or disagrees.
const COMM_LEN: usize = libc::MAXCOMLEN - 1;

/// A start name as read: its length in the buffer, and the length at which its source cuts a longer name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Name {
    len: usize,
    cut: usize,
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

    fn name(&mut self, pid: i32, buffer: &mut [u8; NAME_LEN]) -> io::Result<Name> {
        let info = bsdinfo(pid)?;
        Ok(pick_name(&info.pbi_name.map(|byte| byte as u8), &info.pbi_comm.map(|byte| byte as u8), buffer))
    }
}

fn bsdinfo(pid: i32) -> io::Result<libc::proc_bsdinfo> {
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
    Ok(info)
}

/// The bytes of a C name field up to its NUL.
#[cfg(test)]
fn cstr(field: &[libc::c_char]) -> Vec<u8> {
    field.iter().map(|byte| *byte as u8).take_while(|byte| *byte != 0).collect()
}

/// The start name: `pbi_comm`'s (the kernel's cut, `COMM_LEN`), or the longer `pbi_name` where it agrees with it.
/// A process can rewrite its own `pbi_name` after it starts (`setprogname` writes the `kern.procname` sysctl; nothing
/// but exec writes `pbi_comm`), so `pbi_name` is trusted only when it equals a `pbi_comm` shorter than the cut, or
/// extends a `pbi_comm` of exactly the cut. Anything else is read from `pbi_comm`. Known limit: a rename that keeps the
/// first 15 bytes (or the whole name, when it is shorter) isn't seen, and a name at the cut is a prefix to compare.
fn pick_name(pbi_name: &[u8], pbi_comm: &[u8], buffer: &mut [u8; NAME_LEN]) -> Name {
    let until_nul = |bytes: &[u8]| bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
    let comm = &pbi_comm[..until_nul(pbi_comm).min(COMM_LEN)];
    let long = &pbi_name[..until_nul(pbi_name).min(NAME_LEN)];
    let agrees = if comm.len() < COMM_LEN { long == comm } else { long.starts_with(comm) };
    let (source, cut) = if agrees && !long.is_empty() { (long, NAME_LEN) } else { (comm, COMM_LEN) };
    buffer[..source.len()].copy_from_slice(source);
    Name { len: source.len(), cut }
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
/// cuts the name at `NAME_LEN` bytes (`COMM_LEN` when `pbi_comm` is what was read): a shorter name must differ from the
/// wanted file name in full; a name of exactly the cut may be a cut, so it is skipped only when it is not a prefix of
/// the wanted name. A name that can't be read stays unknown, except that a process that vanished (`ESRCH`) is skipped as `path` skips it.
/// Known limit: the start name is the last part of the path the program was started through, so the app's own executable
/// run through a link of another name, and then deleted while running, would be skipped. The app as launched normally
/// runs under its own name, so a running copy of it stays unknown. A second limit: a program that renames itself
/// (`setprogname`) while keeping the first 15 bytes of its start name, or all of it when it is shorter, reads as the
/// start name or its extension; one that renames itself to anything else is still read by `pbi_comm`.
fn started_as(wanted: &Path, scan: &mut impl Scan, pid: i32) -> Start {
    let mut name = [0u8; NAME_LEN];
    let Name { len: count, cut } = match scan.name(pid, &mut name) {
        Ok(name) => name,
        Err(error) if error.raw_os_error() == Some(libc::ESRCH) => return Start::Other,
        Err(_) => return Start::Unknown("unreadable name".into()),
    };
    let Some(read) = name.get(..count).filter(|read| !read.is_empty()) else {
        return Start::Unknown("unreadable name".into());
    };
    let other = wanted.file_name().is_some_and(|wanted| {
        let wanted = wanted.as_bytes();
        if count == cut { !wanted.starts_with(read) } else { read != wanted }
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
        /// The length at which the name's source cuts, as `Name::cut`.
        cut: usize,
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
        fn name(&mut self, _: i32, buffer: &mut [u8; NAME_LEN]) -> io::Result<Name> {
            let name = self
                .started_as
                .ok_or_else(|| io::Error::from_raw_os_error(self.name_error.unwrap_or(libc::EPERM)))?;
            buffer[..name.len()].copy_from_slice(name);
            Ok(Name {
                len: name.len(),
                cut: self.cut,
            })
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
            cut: NAME_LEN,
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

    /// Failure: the kernel keeps at most 31 bytes of a start name in `pbi_name`, so a running, deleted copy of an app
    /// with a longer executable name is skipped as "another program" and the installer replaces a running app. A cut
    /// name that is a prefix of the wanted name stays unknown; a shorter one is compared in full, with no prefix guess.
    #[test]
    fn a_start_name_cut_by_the_kernel_is_compared_as_a_prefix() {
        let long = Path::new("/private/app/A-very-long-program-name-that-the-kernel-cuts");
        let mut fake = Fake {
            list_bytes: 4,
            path: b"",
            error: Some(libc::ENOENT),
            started_as: Some(b"A-very-long-program-name-that-"),
            name_error: None,
            cut: NAME_LEN,
        };
        assert_eq!(fake.started_as.unwrap().len(), NAME_LEN - 1);
        assert!(
            !observe(long, &mut fake).unwrap(),
            "a name below the cut is compared in full, with no prefix guess"
        );
        fake.started_as = Some(b"A-very-long-program-name-that-t");
        assert_eq!(fake.started_as.unwrap().len(), NAME_LEN);
        assert!(
            observe(long, &mut fake).is_err(),
            "a cut-length name that is a prefix of the wanted name is unknown"
        );
        fake.started_as = Some(b"Another-long-program-name-that-");
        assert!(!observe(long, &mut fake).unwrap(), "a cut-length name that is not a prefix is another program");
        let exactly = Path::new("/private/app/Exactly-31-bytes-long-name-xxxxx");
        fake.started_as = Some(b"Exactly-31-bytes-long-name-xxxx");
        assert!(observe(exactly, &mut fake).is_err(), "a cut-length prefix of the wanted name is unknown");
        let short = Path::new("/private/app/Short");
        fake.started_as = Some(b"Shor");
        assert!(!observe(short, &mut fake).unwrap(), "a short name below the cut is compared in full");
        fake.started_as = Some(b"Short");
        assert!(observe(short, &mut fake).is_err(), "the same short name is unknown");
        fake.started_as = Some(b"A-very-long-program-name-that-t");
        assert!(
            !observe(short, &mut fake).unwrap(),
            "a cut-length name can't be a prefix of a shorter wanted name"
        );
        fake.started_as = None;
        fake.name_error = Some(libc::ESRCH);
        assert!(
            !observe(short, &mut fake).unwrap(),
            "a process that vanished between its path and its name is gone"
        );
        fake.name_error = Some(libc::EPERM);
        assert!(observe(short, &mut fake).is_err(), "any other name error is unknown");
    }

    /// Failure: a 20-byte start name, which `pbi_comm` cuts to 15 and so had to be guessed by prefix, is again treated as
    /// a possible cut: a different wanted name sharing its first 15 bytes keeps an unknown, and the installer is held up
    /// by an unrelated program (or, the other way, a name that differs after byte 15 is judged equal).
    #[test]
    fn a_name_pbi_comm_would_cut_is_an_exact_match_now() {
        let twenty = b"Recto-sibling-tool-x"; // 20 bytes
        assert_eq!(twenty.len(), 20);
        let mut buffer = [0u8; NAME_LEN];
        let name = pick_name(twenty, &twenty[..COMM_LEN], &mut buffer);
        assert_eq!(name, Name { len: 20, cut: NAME_LEN });
        assert_eq!(&buffer[..name.len], twenty);
        let mut fake = Fake {
            list_bytes: 4,
            path: b"",
            error: Some(libc::ENOENT),
            started_as: Some(twenty),
            name_error: None,
            cut: NAME_LEN,
        };
        let wanted = Path::new("/private/app/Recto-sibling-tool-y");
        assert!(
            !observe(wanted, &mut fake).unwrap(),
            "the same first 15 bytes, a different name: another program"
        );
        let same = Path::new("/private/app/Recto-sibling-tool-x");
        assert!(observe(same, &mut fake).is_err(), "the whole name is the same: unknown");
    }

    /// Failure: an empty `pbi_name` leaves the start name empty ("unreadable", so every such program holds up the
    /// update), or the `pbi_comm` fallback is trusted as a whole name when it is a 15-byte cut.
    #[test]
    fn an_empty_pbi_name_falls_back_to_pbi_comm_with_its_own_cut() {
        let mut buffer = [0u8; NAME_LEN];
        let mut comm = [0u8; libc::MAXCOMLEN];
        comm[..COMM_LEN].copy_from_slice(b"Recto-sibling-t");
        let name = pick_name(&[0; 32], &comm, &mut buffer);
        assert_eq!(name, Name { len: COMM_LEN, cut: COMM_LEN });
        assert_eq!(&buffer[..name.len], b"Recto-sibling-t");
        let mut fake = Fake {
            list_bytes: 4,
            path: b"",
            error: Some(libc::ENOENT),
            started_as: Some(b"Recto-sibling-t"),
            name_error: None,
            cut: COMM_LEN,
        };
        let wanted = Path::new("/private/app/Recto-sibling-tool-y");
        assert!(
            observe(wanted, &mut fake).is_err(),
            "a 15-byte fallback name that is a prefix is a possible cut"
        );
        // A non-empty pbi_name that doesn't agree with pbi_comm is not the start name (it can be rewritten).
        let mut other = [0u8; 2 * libc::MAXCOMLEN];
        other[..4].copy_from_slice(b"abcd");
        assert_eq!(pick_name(&other, &comm, &mut buffer), Name { len: COMM_LEN, cut: COMM_LEN });
    }

    /// Failure: `pbi_name` can be rewritten by the process itself after it starts (`setprogname` calls the `kern.procname`
    /// sysctl; `pbi_comm` can't be), so a running app that renamed itself reads as "another program", is skipped, and the
    /// installer replaces it while it is open. The start name is trusted only where `pbi_name` agrees with `pbi_comm`.
    #[test]
    fn a_pbi_name_that_disagrees_with_pbi_comm_is_not_trusted() {
        let comm = |name: &[u8]| {
            let mut field = [0u8; libc::MAXCOMLEN];
            field[..name.len()].copy_from_slice(name);
            field
        };
        let name = |name: &[u8]| {
            let mut field = [0u8; 2 * libc::MAXCOMLEN];
            field[..name.len()].copy_from_slice(name);
            field
        };
        let read = |pbi_name: &[u8], pbi_comm: &[u8]| {
            let mut buffer = [0u8; NAME_LEN];
            let got = pick_name(&name(pbi_name), &comm(pbi_comm), &mut buffer);
            (buffer[..got.len].to_vec(), got.cut)
        };
        // Renamed by the process: the start name is pbi_comm's, and the app stays unknown.
        assert_eq!(read(b"renamed", b"RectoPDF"), (b"RectoPDF".to_vec(), COMM_LEN));
        let mut fake = Fake {
            list_bytes: 4,
            path: b"",
            error: Some(libc::ENOENT),
            started_as: Some(b"RectoPDF"),
            name_error: None,
            cut: COMM_LEN,
        };
        assert!(observe(Path::new("/private/app/RectoPDF"), &mut fake).is_err(), "a renamed app stays unknown");
        // Agreeing: a short name that equals, a 15-byte comm that pbi_name extends.
        assert_eq!(read(b"RectoPDF", b"RectoPDF"), (b"RectoPDF".to_vec(), NAME_LEN));
        let long = b"A-very-long-program-name";
        assert_eq!(read(long, &long[..COMM_LEN]), (long.to_vec(), NAME_LEN));
        // A 15-byte comm with a pbi_name that doesn't extend it, or a short comm that pbi_name merely extends: comm.
        assert_eq!(read(b"Another-long-program", &long[..COMM_LEN]), (long[..COMM_LEN].to_vec(), COMM_LEN));
        assert_eq!(read(b"RectoPDF-more", b"RectoPDF"), (b"RectoPDF".to_vec(), COMM_LEN));
        // A comm field filled to its last byte (no NUL) is the 15 bytes the kernel keeps.
        let mut full = [b'a'; libc::MAXCOMLEN];
        full[COMM_LEN] = b'Z';
        let mut buffer = [0u8; NAME_LEN];
        assert_eq!(pick_name(&name(&full[..COMM_LEN]), &full, &mut buffer), Name { len: COMM_LEN, cut: NAME_LEN });
        assert_eq!(&buffer[..COMM_LEN], &full[..COMM_LEN]);
        assert_eq!(pick_name(&name(b"zzz"), &full, &mut buffer), Name { len: COMM_LEN, cut: COMM_LEN });
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

    /// The stand-in process of `deleted_stand_in`: a copy of this test binary runs this test with `RECTO_STAND_IN`
    /// set, says it is ready and then blocks until its stdin closes. Run on its own it returns at once, and it is
    /// ignored so no suite run starts it.
    #[test]
    #[ignore = "a stand-in process for the deleted-program test, started by it"]
    fn stand_in_program() {
        if std::env::var_os("RECTO_STAND_IN").is_none() {
            return;
        }
        if let Some(new) = std::env::var_os("RECTO_STAND_IN_RENAME") {
            // What `setprogname` does: the process rewrites its own `pbi_name`, not `pbi_comm`.
            let new = std::ffi::CString::new(new.as_bytes()).unwrap();
            // SAFETY: a NUL-terminated name and its exact length for a write of `kern.procname`.
            let set = unsafe {
                libc::sysctlbyname(
                    c"kern.procname".as_ptr(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    new.as_ptr().cast_mut().cast(),
                    new.as_bytes().len(),
                )
            };
            assert_eq!(set, 0, "the stand-in couldn't rename itself: {}", io::Error::last_os_error());
        }
        println!("stand-in ready");
        let _ = io::Write::flush(&mut io::stdout());
        let _ = io::copy(&mut io::stdin(), &mut io::sink());
    }

    /// A copy of this test binary named `name`, started as the stand-in and then deleted once it has said it is
    /// ready: a live process whose image is gone, which the OS answers a path query for with ENOENT. What the rest
    /// of the test relies on is checked here through the real scan, each with its own message, so a stand-in that
    /// never ran, ended, or whose path the OS still answers fails by saying so, not through the rule under test.
    fn deleted_stand_in(name: &str) -> (recto_testpdf::TempDir, Children) {
        deleted_stand_in_renamed(name, None)
    }

    /// As `deleted_stand_in`, with the process renaming itself to `rename` before it is ready.
    fn deleted_stand_in_renamed(name: &str, rename: Option<&str>) -> (recto_testpdf::TempDir, Children) {
        let dir = recto_testpdf::TempDir::new("update-processes-deleted-");
        let copy = dir.path().join(name);
        std::fs::copy(std::env::current_exe().unwrap(), &copy).unwrap();
        let mut child = std::process::Command::new(&copy)
            .args(["local::update_processes::tests::stand_in_program", "--exact", "--ignored", "--nocapture"])
            .arg("--test-threads=1")
            .env("RECTO_STAND_IN", "1")
            .envs(rename.map(|new| ("RECTO_STAND_IN_RENAME", new)))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut ready = false;
        let mut lines = std::io::BufReader::new(child.stdout.as_mut().unwrap());
        let mut line = String::new();
        while !ready && io::BufRead::read_line(&mut lines, &mut line).unwrap() > 0 {
            // libtest prints "test <name> ... " before the test body writes, on the same line.
            ready = line.trim_end().ends_with("stand-in ready");
            line.clear();
        }
        let pid = child.id() as i32;
        if !ready {
            let status = child.wait();
            panic!("the stand-in never ran: {status:?}");
        }
        let mut children = Children(vec![child]);
        std::fs::remove_file(&copy).unwrap();
        let status = children.0[0].try_wait().unwrap();
        assert!(status.is_none(), "the stand-in ended: {status:?}");
        let mut buffer = [0u8; MAX_PATH];
        let result = Mac.path(pid, &mut buffer);
        assert!(
            result.as_ref().is_err_and(|error| error.raw_os_error() == Some(libc::ENOENT)),
            "the OS doesn't answer ENOENT for a live program whose file was deleted here: {:?}",
            result.map(|count| String::from_utf8_lossy(&buffer[..count]).into_owned())
        );
        let info = bsdinfo(pid).unwrap();
        let (comm, procname) = (cstr(&info.pbi_comm), cstr(&info.pbi_name));
        match rename {
            Some(new) => assert_eq!(procname, new.as_bytes(), "the rename didn't reach pbi_name"),
            None => assert_eq!(procname, &name.as_bytes()[..name.len().min(NAME_LEN)], "pbi_name isn't the start name"),
        }
        assert_eq!(comm, &name.as_bytes()[..name.len().min(COMM_LEN)], "a rename must not reach pbi_comm");
        let mut started = [0u8; NAME_LEN];
        let read = Mac
            .name(pid, &mut started)
            .unwrap_or_else(|error| panic!("the stand-in's name can't be read: {error}"));
        assert_eq!(
            &started[..read.len],
            &name.as_bytes()[..name.len().min(NAME_LEN)],
            "the scan reads another start name than the copy's"
        );
        (dir, children)
    }

    /// Failure: an unrelated program of this user whose file was deleted (an updated tool still running) makes
    /// every observation of the update executable unknown, so an update can't go ahead (owner, 2026-11-25,
    /// `docs/OWNER-RULES.md` "Decisions of 2026-11-25"). A deleted program that could still be RectoPDF must
    /// stay unknown, so RectoPDF is never replaced while it might be open.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_deleted_program_started_under_another_name_is_skipped_but_one_named_like_the_app_is_not() {
        // A name that isn't the app's, short enough for the kernel to keep whole.
        let other = format!("rpdf-o-{}", std::process::id());
        assert!(other.len() < NAME_LEN);
        let (_other, _children) = deleted_stand_in(&other);
        assert!(update_app_is_running(&std::env::current_exe().unwrap()).unwrap());

        // A name over the 15 bytes `pbi_comm` keeps: read whole from `pbi_name`, so a wanted file that shares its first 15
        // bytes but is another name is no possible match (it was an unknown by prefix before).
        let long = format!("rpdf-long-name-{}", std::process::id());
        assert!(long.len() > COMM_LEN && long.len() < NAME_LEN);
        let (_long, _more_children) = deleted_stand_in(&long);
        let dir = recto_testpdf::TempDir::new("update-processes-wanted-");
        let sibling = dir.path().join("rpdf-long-name-sibling");
        std::fs::File::create(&sibling).unwrap();
        assert!(
            !update_app_is_running(&sibling).unwrap(),
            "a name over 15 bytes is compared whole, not by prefix"
        );
        // A name of this run only: a deleted copy under the app's real name would make every other installer test, and
        // a real updater on this Mac, see an unknown while it lives. The rule under test is "the same name".
        let unique = format!("rpdf-{}", std::process::id());
        assert!(unique.len() < NAME_LEN);
        let wanted = dir.path().join(&unique);
        std::fs::File::create(&wanted).unwrap();
        let (_same_name, _more) = deleted_stand_in(&unique);
        let error = update_app_is_running(&wanted).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound, "{error}");
        assert!(
            error.to_string().contains("has a deleted executable"),
            "the unknown says which process: {error}"
        );
    }

    /// Failure: a running app that renamed itself (`setprogname`) is read as "another program" through `pbi_name` and the
    /// installer replaces it while it is open. Known limit, said here: a rename that keeps the first 15 bytes isn't seen.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_deleted_program_that_renamed_itself_is_still_read_by_its_start_name() {
        let dir = recto_testpdf::TempDir::new("update-processes-renamed-");
        let unique = format!("rpdf-r-{}", std::process::id());
        let wanted = dir.path().join(&unique);
        std::fs::File::create(&wanted).unwrap();
        let (_copy, _children) = deleted_stand_in_renamed(&unique, Some("renamed-itself"));
        let error = update_app_is_running(&wanted).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound, "{error}");
        assert!(error.to_string().contains(&unique), "the unknown names the start name: {error}");
    }

    /// Failure: the cut lengths are assumed, not seen: if `pbi_name` kept fewer than 31 bytes or `pbi_comm` more than 15
    /// on a macOS, a long name would be called "another program" or an unrelated one an unknown.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_name_over_31_bytes_is_cut_at_31_in_pbi_name_and_at_15_in_pbi_comm() {
        let name = format!("rpdf-over-31-bytes-name-for-pbi-name-{}", std::process::id());
        assert!(name.len() > NAME_LEN);
        // `deleted_stand_in` asserts pbi_name = the first 31 bytes, pbi_comm = the first 15 and the scan's Name.
        let (dir, _children) = deleted_stand_in(&name);
        let mut buffer = [0u8; NAME_LEN];
        let read = Mac.name(_children.0[0].id() as i32, &mut buffer).unwrap();
        assert_eq!(read, Name { len: NAME_LEN, cut: NAME_LEN });
        // The wanted file shares the 31 bytes the OS keeps: it may be the same program. One that differs inside them isn't.
        let same = dir.path().join(format!("{name}-and-more"));
        std::fs::File::create(&same).unwrap();
        assert!(update_app_is_running(&same).is_err(), "the 31 bytes kept are a prefix of the wanted name");
        let other = dir.path().join(format!("rpdf-over-31-bytes-name-for-pbX-{}", std::process::id()));
        std::fs::File::create(&other).unwrap();
        assert!(!update_app_is_running(&other).unwrap());
    }

    /// Failure: the native ABI or path handling silently misses this process.
    #[test]
    fn native_observation_finds_the_current_program() {
        assert!(update_app_is_running(&std::env::current_exe().unwrap()).unwrap());
        assert!(update_app_is_running(Path::new("/no/such/update/program")).is_err());
    }
}
