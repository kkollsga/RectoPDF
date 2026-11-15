//! **The key the Find text is sealed with** (owner, 2026-11-15: "Find text is a indexed copy of the pdf text? Lets store it encrypted"). 32 random bytes, held in memory only while a session or the indexer that was handed it lives, overwritten when it goes, never printed, logged, cloned or serialised, and never answered to any command.
//!
//! Where it comes from is decided once, by whoever starts a session, and said by [`Source`]:
//! - **The Keychain**, for a copy of the app signed by the team, and only the app's own process: [`keychain`]. Every call refuses interaction, so nothing ever prompts.
//! - **A file in a test state folder**, for quiet runs and tests, which must never touch the person's Keychain: [`FindKey::from_state_file`]. Only a state folder named by `RECTO_STATE_DIR` can get one, by the caller's choice, and the file is the state folder's own.
//! - **Memory**, for everything else and whenever the Keychain can't be had: a development build, a locked or refusing Keychain, an item this copy can't read. What's sealed with it can't be read by the next run, which removes it and builds again when asked.
//!
//! The indexer gets the key from the session on its stdin, as 32 raw bytes before its job: never in its arguments, its environment or a file. [`FindKey::write_to`] and [`FindKey::read_from`] are the two ends.

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

    /// The key kept in a test state folder's own file, made the first time. A file that's there and isn't exactly a key, or isn't private to this user, is refused, never fixed: the caller runs on a key in memory.
    ///
    /// # Errors
    ///
    /// As above, and when the file can't be read or made.
    pub fn from_state_file(path: &Path) -> io::Result<Self> {
        if let Some(mut stored) = crate::local::read_private_file(path, KEY_BYTES as u64 + 1)? {
            let read = Self::from_slice(&stored, Source::StateFile);
            crate::secret::wipe(std::mem::take(&mut stored));
            return read.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "not a key"));
        }
        let key = Self::fresh(Source::StateFile)?;
        // Made new, so a link planted at the name is never followed, and two runs starting at once can't both write: the loser reads the winner's.
        match crate::local::create_private_file(path) {
            Ok(mut file) => {
                file.write_all(key.bytes.as_slice())?;
                file.sync_all()?;
                Ok(key)
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Self::from_state_file(path),
            Err(error) => Err(error),
        }
    }

    fn from_slice(bytes: &[u8], source: Source) -> Option<Self> {
        let bytes: &[u8; KEY_BYTES] = bytes.try_into().ok()?;
        Some(Self {
            bytes: Box::new(*bytes),
            source,
        })
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

    /// A key a test makes up. Never for anything that's kept.
    pub fn for_tests(byte: u8) -> Self {
        Self {
            bytes: Box::new([byte; KEY_BYTES]),
            source: Source::Memory,
        }
    }
}

/// **The app's own key, and why it isn't the Keychain's when it isn't.** The Keychain is asked only by a copy signed by `team`; any refusal, and every other copy, gets a key in memory, with the refusal's code. `None` only when the OS gives no randomness, and then nothing is indexed. The one caller is the app's session, on the indexer's thread: never the UI thread, `recto`, a helper, or a test state folder.
pub fn load_for_the_app(team: &str) -> Option<(FindKey, Option<&'static str>)> {
    let refused = if keychain::signed_by_team(team) {
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

/// For a process that holds the key or pages' text: a crash leaves no image of its memory. macOS writes none unless the limit's raised, and this lowers it for good whatever the shell set. A crash report still carries registers and the stack's addresses, not memory.
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
        self.bytes.fill(0);
        std::hint::black_box(&self.bytes);
    }
}

impl std::fmt::Debug for FindKey {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "FindKey({})", self.source.code())
    }
}

#[cfg_attr(target_os = "macos", path = "find_key_mac.rs")]
#[cfg_attr(not(target_os = "macos"), path = "find_key_stub.rs")]
pub mod keychain;

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

    /// Failure: a test state folder's key changes from one run to the next, so every quiet run's corpus is unreadable to the next; or a file that isn't a key, or anyone else's, is used.
    #[test]
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
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(FindKey::from_state_file(&path).is_ok(), "readable by others is still this user's own file");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(FindKey::from_state_file(&path).is_err(), "a key anyone can write was used");
        std::fs::remove_file(&path).unwrap();
        crate::local::write_private_file(&path, &[7; 31]).unwrap();
        assert!(FindKey::from_state_file(&path).is_err(), "31 bytes were taken for a key");
        let _ = std::fs::remove_dir_all(&temp);
    }
}
