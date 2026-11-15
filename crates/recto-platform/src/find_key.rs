//! **The key the Find text is sealed with** (owner, 2026-11-15: "Find text is a indexed copy of the pdf text? Lets store it encrypted"). 32 random bytes, held in memory only while a session or the indexer that was handed it lives, overwritten when it goes, never printed, logged, cloned or serialised, and never answered to any command.
//!
//! Where it comes from is decided once, by whoever starts a session, and said by [`Source`]:
//! - **The Keychain**, for a copy of the app that is the team's release, checked whole, and only the app's own process: [`keychain`]. Interaction is refused for the whole process before the first call, so nothing prompts.
//! - **A file in a test state folder**, for quiet runs and tests, which must never touch the person's Keychain: [`FindKey::from_state_file`]. Only a state folder named by `RECTO_STATE_DIR` can get one, by the caller's choice, and the file is the state folder's own.
//! - **Memory**, for everything else and whenever the Keychain can't be had: a development build, a locked or refusing Keychain, an item this copy can't read. What's sealed with it can't be read by the next run, which removes it and builds again when asked.
//!
//! The indexer gets the key from the session on its stdin, as 32 raw bytes before its job: never in its arguments, its environment or a file. [`FindKey::write_to`] and [`FindKey::read_from`] are the two ends.
//!
//! **What's overwritten, and what isn't** (as true as Rust lets this be said): the key's own 32 bytes, in one box they're written into in place (from the OS's randomness, the pipe, the Keychain's data or a key file's bytes, never through a copy on the stack) and never moved, and a key file's bytes once read, are overwritten when they go (`zeroize`). Not: the copy CoreFoundation makes to add the key to the Keychain or to hand it back, which is its memory to free ([`keychain`]); the bytes in the pipe's kernel buffer until the helper reads them; and, from slice 2, what HKDF and AES derive from it on their own stacks beyond what `aes-gcm`'s `zeroize` covers. A process that can read this one's memory has the key anyway.

use std::io::{self, Read, Write};
use std::path::Path;

/// How long a key is.
pub const KEY_BYTES: usize = 32;

/// Where this run's key came from. Said in `qa state` and `index status`, and nothing about the key itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Keychain,
    StateFile,
    Memory,
    /// Handed over by the session that started this process.
    Handed,
}

impl Source {
    pub fn code(self) -> &'static str {
        match self {
            Self::Keychain => "keychain",
            Self::StateFile => "state_file",
            Self::Memory => "memory",
            Self::Handed => "handed",
        }
    }

    /// Whether what's sealed with it can be read by a later run.
    pub fn lasts(self) -> bool {
        matches!(self, Self::Keychain | Self::StateFile)
    }
}

/// The key. See the module's notes.
pub struct FindKey {
    bytes: Box<[u8; KEY_BYTES]>,
    source: Source,
}

impl FindKey {
    /// A new key for this run alone.
    ///
    /// # Errors
    ///
    /// When the OS gives no randomness, which leaves nothing to seal with: the caller indexes nothing.
    pub fn in_memory() -> io::Result<Self> {
        Self::fresh(Source::Memory)
    }

    fn fresh(source: Source) -> io::Result<Self> {
        let mut bytes = Box::new([0u8; KEY_BYTES]);
        getrandom::fill(bytes.as_mut_slice()).map_err(|_| io::Error::other("no randomness"))?;
        Ok(Self { bytes, source })
    }

    /// The key kept in a test state folder's own file, made the first time. A file that's there and isn't exactly 32 bytes, or that anyone but this user may read or write (any bit of group or other in its mode), or isn't a plain file of theirs, is refused, never fixed: the caller runs on a key in memory.
    ///
    /// A new key is written whole to a name of its own, synced, and only then given the key file's name, in one step that fails if anything's there already (`renamex_np` with `RENAME_EXCL`): so a run reading at the same moment sees no file or the whole key, never part of one, two runs starting at once can't both write (the loser reads the winner's), and a write that fails leaves nothing behind.
    ///
    /// # Errors
    ///
    /// As above, and when the file can't be read or made.
    pub fn from_state_file(path: &Path) -> io::Result<Self> {
        match crate::local::open_private_file(path) {
            Ok(file) => return Self::read_key_file(file),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let key = Self::fresh(Source::StateFile)?;
        let temp = path.with_extension(format!("{}.{:016x}.tmp", std::process::id(), crate::local::random_u64()));
        let written = crate::local::create_private_file(&temp).and_then(|mut file| {
            file.write_all(key.bytes.as_slice())?;
            file.sync_all()
        });
        let placed = written.and_then(|()| rename_if_absent(&temp, path));
        match placed {
            Ok(()) => Ok(key),
            Err(error) => {
                let _ = std::fs::remove_file(&temp);
                if error.kind() == io::ErrorKind::AlreadyExists {
                    return Self::from_state_file(path);
                }
                Err(error)
            }
        }
    }

    fn read_key_file(file: std::fs::File) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if file.metadata()?.permissions().mode() & 0o077 != 0 {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "a key others may read or write"));
            }
        }
        let mut stored = Vec::with_capacity(KEY_BYTES + 1);
        file.take(KEY_BYTES as u64 + 1).read_to_end(&mut stored)?;
        let read = Self::from_slice(&stored, Source::StateFile);
        zeroize::Zeroize::zeroize(&mut stored);
        read.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "not a key"))
    }

    /// Exactly 32 bytes, copied straight into the key's own box.
    pub(crate) fn from_slice(bytes: &[u8], source: Source) -> Option<Self> {
        if bytes.len() != KEY_BYTES {
            return None;
        }
        let mut key = Self {
            bytes: Box::new([0u8; KEY_BYTES]),
            source,
        };
        key.bytes.copy_from_slice(bytes);
        Some(key)
    }

    /// What's sealed with this. Only the sealing code asks.
    pub fn bytes(&self) -> &[u8; KEY_BYTES] {
        &self.bytes
    }

    pub fn source(&self) -> Source {
        self.source
    }

    /// Hands the key to a helper: 32 raw bytes on its stdin, before anything else is written there.
    ///
    /// # Errors
    ///
    /// When the pipe is closed.
    pub fn write_to(&self, pipe: &mut impl Write) -> io::Result<()> {
        pipe.write_all(self.bytes.as_slice())
    }

    /// The helper's end: exactly 32 bytes, read straight into the key's own memory, so no buffer holds a copy. `stdin` must be read unbuffered for that: [`read_from_stdin`].
    ///
    /// # Errors
    ///
    /// When fewer than 32 bytes come.
    pub fn read_from(pipe: &mut impl Read) -> io::Result<Self> {
        let mut key = Self {
            bytes: Box::new([0u8; KEY_BYTES]),
            source: Source::Handed,
        };
        pipe.read_exact(key.bytes.as_mut_slice())?;
        Ok(key)
    }

    /// A key a test makes up. Never for anything that's kept: only built for this crate's tests and with `test-seams`, which nothing shipped turns on.
    #[cfg(any(test, feature = "test-seams"))]
    pub fn for_tests(byte: u8) -> Self {
        Self {
            bytes: Box::new([byte; KEY_BYTES]),
            source: Source::Memory,
        }
    }
}

/// **The app's own key, and why it isn't the Keychain's when it isn't.** The Keychain is asked only by a copy that is `team`'s release, checked whole ([`keychain`]); any refusal, and every other copy, gets a key in memory, with the refusal's code. `None` only when the OS gives no randomness, and then nothing is indexed. The only way to the Keychain from outside this crate. Its one caller is `recto_control::find_keys::AppKey`, made only by the app's start; the indexer takes it on no main thread.
pub fn load_for_the_app(team: &str) -> Option<(FindKey, Option<&'static str>)> {
    let refused = if keychain::signed_as_a_release(team) {
        match keychain::load_or_create(keychain::SERVICE, keychain::ACCOUNT) {
            Ok(key) => return Some((key, None)),
            Err(refused) => refused.code(),
        }
    } else {
        keychain::Refused::NotTheTeam.code()
    };
    FindKey::in_memory().ok().map(|key| (key, Some(refused)))
}

/// The key from this process's stdin, read with no buffer in between: `std::io::stdin()` reads ahead into a buffer of its own, which would keep a copy of the key for the life of the process. Call it before anything else reads stdin.
///
/// # Errors
///
/// When fewer than 32 bytes come.
#[cfg(unix)]
pub fn read_from_stdin() -> io::Result<FindKey> {
    use std::os::fd::FromRawFd;
    // SAFETY: descriptor 0 is this process's stdin for its whole life. `ManuallyDrop` keeps this `File` from closing it.
    let mut stdin = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(0) });
    FindKey::read_from(&mut *stdin)
}

#[cfg(not(unix))]
pub fn read_from_stdin() -> io::Result<FindKey> {
    FindKey::read_from(&mut io::stdin())
}

/// For a process that holds the key or pages' text: a crash leaves no image of its memory. No caller yet: the indexer calls it once the Find text is sealed (slice 2). macOS writes none unless the limit's raised, and this lowers it for good whatever the shell set. A crash report still carries registers and the stack's addresses, not memory.
pub fn forbid_core_dumps() {
    #[cfg(target_os = "macos")]
    {
        let none = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // SAFETY: a plain call with a value on the stack.
        let _ = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &none) };
    }
}

impl Drop for FindKey {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(self.bytes.as_mut_slice());
    }
}

impl std::fmt::Debug for FindKey {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "FindKey({})", self.source.code())
    }
}

/// Gives `temp` the name `to`, in one step that fails with `AlreadyExists` if anything's at `to`.
#[cfg(target_os = "macos")]
fn rename_if_absent(temp: &Path, to: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let (from, to) = (
        std::ffi::CString::new(temp.as_os_str().as_bytes())?,
        std::ffi::CString::new(to.as_os_str().as_bytes())?,
    );
    // SAFETY: two NUL-terminated paths, valid for the call.
    if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "macos"))]
fn rename_if_absent(temp: &Path, to: &Path) -> io::Result<()> {
    std::fs::hard_link(temp, to)?;
    std::fs::remove_file(temp)
}

#[cfg_attr(target_os = "macos", path = "find_key_mac.rs")]
#[cfg_attr(not(target_os = "macos"), path = "find_key_stub.rs")]
mod keychain;

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a key can be printed, or two runs get the same key from nothing, or a key handed over arrives as something else.
    #[test]
    fn a_key_hides_itself_and_crosses_a_pipe_whole() {
        let key = FindKey::in_memory().unwrap();
        assert_eq!(format!("{key:?}"), "FindKey(memory)");
        assert_ne!(key.bytes(), FindKey::in_memory().unwrap().bytes(), "two fresh keys are the same");
        let mut pipe = Vec::new();
        key.write_to(&mut pipe).unwrap();
        pipe.extend_from_slice(b"{\"job\":1}\n");
        let mut reader = pipe.as_slice();
        let handed = FindKey::read_from(&mut reader).unwrap();
        assert_eq!(handed.bytes(), key.bytes());
        assert_eq!(handed.source(), Source::Handed);
        assert_eq!(reader, b"{\"job\":1}\n", "the job after the key is left for the job's reader");
        assert!(FindKey::read_from(&mut &[1u8; 31][..]).is_err(), "a short key was taken");
    }

    /// Failure: a test state folder's key changes from one run to the next, so every quiet run's corpus is unreadable to the next; or a file that isn't exactly a key, or that anyone else may read or write, is used; or making the key leaves a half-written file a racer could read, or a temp file behind.
    #[test]
    #[cfg(unix)]
    fn a_state_folders_key_lasts_and_a_wrong_file_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let temp = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("recto-find-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        crate::local::ensure_private_dir(&temp).unwrap();
        let path = temp.join("find.key");
        let first = FindKey::from_state_file(&path).unwrap();
        assert_eq!(first.source(), Source::StateFile);
        assert_eq!(FindKey::from_state_file(&path).unwrap().bytes(), first.bytes());
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let names: Vec<_> = std::fs::read_dir(&temp).unwrap().flatten().map(|entry| entry.file_name()).collect();
        assert_eq!(names, [std::ffi::OsString::from("find.key")], "a temp file was left beside the key");
        for mode in [0o640, 0o604, 0o620, 0o666] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(FindKey::from_state_file(&path).is_err(), "a key of mode {mode:o} was used");
        }
        for length in [31, 33, 64] {
            std::fs::remove_file(&path).unwrap();
            crate::local::write_private_file(&path, &vec![7; length]).unwrap();
            assert!(FindKey::from_state_file(&path).is_err(), "{length} bytes were taken for a key");
        }
        let _ = std::fs::remove_dir_all(&temp);
    }
}

/// **What a built program holds of the Keychain**, for the tests that hold `recto` and every helper to having none: the framework's path this module opens it by (the code that reaches the Keychain at all), a load command for Security.framework (`otool -L`), and an import of any `_Sec…` function (`nm -u`). Each finding is said in words; none is nothing. Fails outright if the program or a tool to look at it is missing, so a check can't pass by looking at nothing.
#[cfg(any(test, feature = "test-seams"))]
pub mod audit {
    use std::path::Path;

    fn tool(program: &str, args: &[&str], on: &Path) -> String {
        let out = std::process::Command::new(program)
            .args(args)
            .arg(on)
            .output()
            .unwrap_or_else(|error| panic!("{program} can't be run: {error}"));
        assert!(out.status.success(), "{program} {}: {}", on.display(), String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Everything in `program` that reaches the Keychain, said in words.
    #[must_use]
    pub fn keychain_code_in(program: &Path) -> Vec<String> {
        let bytes = std::fs::read(program).unwrap_or_else(|error| panic!("{}: {error}", program.display()));
        let mut found = Vec::new();
        let path = b"Security.framework/Security";
        if bytes.windows(path.len()).any(|window| window == path) {
            found.push("names Security.framework's path, as the Keychain's code opens it".to_string());
        }
        for line in tool("/usr/bin/otool", &["-L"], program).lines().skip(1) {
            if line.contains("Security.framework") {
                found.push(format!("links {}", line.trim()));
            }
        }
        for line in tool("/usr/bin/nm", &["-u"], program).lines() {
            if line.trim().starts_with("_Sec") {
                found.push(format!("imports {}", line.trim()));
            }
        }
        found
    }
}
