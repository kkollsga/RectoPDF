//! **The key the Find text is sealed with** (owner, 2026-11-15: "Find text is a indexed copy of the pdf text? Lets store it encrypted"). 32 random bytes, held in memory only while a session or the indexer that was handed it lives, overwritten when it goes, never printed, logged, cloned or serialised, and never answered to any command.
//!
//! Where it comes from is decided once, by whoever starts a session, and said by [`Source`]:
//! - **The Keychain**, for a copy of the app that is the team's release, checked whole, and only the app's own process: [`keychain`]. Interaction is refused for the whole process before the first call, so nothing prompts.
//! - **A file in a test state folder**, for quiet runs and tests, which must never touch the person's Keychain: [`FindKey::from_state_file`]. Only a state folder named by `RECTO_STATE_DIR` can get one, by the caller's choice, and the file is the state folder's own.
//! - **Memory**, for everything else and whenever the Keychain can't be had: a development build, a locked or refusing Keychain, an item this copy can't read. What's sealed with it can't be read by the next run, which removes it and builds again when asked.
//!
//! **The indexer never gets this key** (the security read of 2026-11-15, S1: a helper swapped in the app's bundle after its check would have been handed the key to every corpus). It gets a key *bound* to the one corpus it's building, derived from this one by `recto_index::seal::grant_for`, which opens and seals that corpus and no other, on its stdin before its job: never in its arguments, its environment or a file. [`FindKey::write_to`] refuses any key that isn't bound, and [`FindKey::read_from`] takes nothing else.
//!
//! **What's overwritten, and what isn't** (as true as Rust lets this be said): the key's own 32 bytes, in one box they're written into in place (from the OS's randomness, the pipe, the Keychain's data or a key file's bytes, never through a copy on the stack) and never moved, and a key file's bytes once read, are overwritten when they go (`zeroize`). Not: the copy CoreFoundation makes to add the key to the Keychain or to hand it back, which is its memory to free ([`keychain`]); the bytes in the pipe's kernel buffer until the helper reads them; and, from slice 2, what HKDF and AES derive from it on their own stacks beyond what `aes-gcm`'s `zeroize` covers. A process that can read this one's memory has the key anyway.

use std::io::{self, Read, Write};
use std::path::Path;

/// How long a key is.
pub const KEY_BYTES: usize = 32;

/// Where this run's key came from. Said in `qa state` and `index status`, and nothing about the key itself. A key handed to a helper keeps its session's source: what it seals is the session's, and lasts as the session's key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Keychain,
    StateFile,
    /// In memory, as this copy can never have a lasting key: not the team's release, or a host with nothing to keep.
    Memory,
    /// In memory for now: a lasting key exists and can't be had this run (the Keychain locked, a read refused, an item not this copy's, a key file that won't read). What's sealed with the lasting key is left as it is.
    MemoryForNow,
}

impl Source {
    pub fn code(self) -> &'static str {
        match self {
            Self::Keychain => "keychain",
            Self::StateFile => "state_file",
            Self::Memory | Self::MemoryForNow => "memory",
        }
    }

    /// Whether what's sealed with it can be read by a later run.
    pub fn lasts(self) -> bool {
        matches!(self, Self::Keychain | Self::StateFile)
    }

    /// Whether a run on this key may throw away and build again over a corpus a lasting key sealed. Not when the lasting key is only out of reach for now: that corpus is the person's, for the next run that has its key.
    pub fn may_replace_lasting(self) -> bool {
        self != Self::MemoryForNow
    }

    /// The one byte it crosses a pipe as.
    fn byte(self) -> u8 {
        match self {
            Self::Memory => 0,
            Self::MemoryForNow => 1,
            Self::Keychain => 2,
            Self::StateFile => 3,
        }
    }

    fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            0 => Self::Memory,
            1 => Self::MemoryForNow,
            2 => Self::Keychain,
            3 => Self::StateFile,
            _ => return None,
        })
    }
}

/// The key. See the module's notes.
pub struct FindKey {
    bytes: Box<[u8; KEY_BYTES]>,
    source: Source,
    /// For a key bound to one corpus, that corpus's key (not secret: it names the corpus, as its folder does), and then `bytes` are that corpus's own root key. `None` for the session's key, from which every corpus's is derived.
    binding: Option<[u8; KEY_BYTES]>,
}

/// The version of what crosses a helper's stdin: a bound key, then its source, then its corpus. Anything else is refused.
const FRAME_VERSION: u8 = 2;

impl FindKey {
    /// A new key for this run alone.
    ///
    /// # Errors
    ///
    /// When the OS gives no randomness, which leaves nothing to seal with: the caller indexes nothing.
    pub fn in_memory() -> io::Result<Self> {
        Self::fresh(Source::Memory)
    }

    /// A key for this run alone, when a lasting key exists and can't be had now: [`Source::MemoryForNow`].
    ///
    /// # Errors
    ///
    /// As [`Self::in_memory`].
    pub fn in_memory_for_now() -> io::Result<Self> {
        Self::fresh(Source::MemoryForNow)
    }

    fn fresh(source: Source) -> io::Result<Self> {
        let mut bytes = Box::new([0u8; KEY_BYTES]);
        getrandom::fill(bytes.as_mut_slice()).map_err(|_| io::Error::other("no randomness"))?;
        Ok(Self { bytes, source, binding: None })
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
            binding: None,
        };
        key.bytes.copy_from_slice(bytes);
        Some(key)
    }

    /// A key bound to one corpus: `root` is that corpus's own root key, derived from the session's key by `recto_index::seal::grant_for`, the only caller, and `corpus` its corpus key. It keeps the session's source, as what it seals is the session's. Copied straight into the key's own box.
    #[must_use]
    pub fn bound(root: &[u8; KEY_BYTES], source: Source, corpus: [u8; KEY_BYTES]) -> Self {
        let mut key = Self {
            bytes: Box::new([0u8; KEY_BYTES]),
            source,
            binding: Some(corpus),
        };
        key.bytes.copy_from_slice(root);
        key
    }

    /// The corpus this key is bound to, if it's bound to one; `None` for a session's key.
    pub fn binding(&self) -> Option<&[u8; KEY_BYTES]> {
        self.binding.as_ref()
    }

    /// What's sealed with this. Only the sealing code asks.
    pub fn bytes(&self) -> &[u8; KEY_BYTES] {
        &self.bytes
    }

    pub fn source(&self) -> Source {
        self.source
    }

    /// Hands a key bound to one corpus to a helper, on its stdin before anything else is written there: the frame's version, the 32 bytes, its source's byte (what the helper seals says whether it lasts), and the 32 bytes of the corpus it's bound to. Never a session's key.
    ///
    /// # Errors
    ///
    /// When the key isn't bound to a corpus, or the pipe is closed.
    pub fn write_to(&self, pipe: &mut impl Write) -> io::Result<()> {
        let Some(corpus) = &self.binding else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "only a key bound to one corpus is handed to a helper",
            ));
        };
        pipe.write_all(&[FRAME_VERSION])?;
        pipe.write_all(self.bytes.as_slice())?;
        pipe.write_all(&[self.source.byte()])?;
        pipe.write_all(corpus)
    }

    /// The helper's end: the frame's version, then 32 bytes read straight into the key's own memory, so no buffer holds a copy, then its source's byte and its corpus. `stdin` must be read unbuffered for that: [`read_from_stdin`]. What it gives is always bound to that corpus.
    ///
    /// # Errors
    ///
    /// When the version isn't this one's, fewer bytes come than the frame has, or the source isn't one.
    pub fn read_from(pipe: &mut impl Read) -> io::Result<Self> {
        let mut version = [0u8; 1];
        pipe.read_exact(&mut version)?;
        if version[0] != FRAME_VERSION {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "not a key frame this helper reads"));
        }
        let mut key = Self {
            bytes: Box::new([0u8; KEY_BYTES]),
            source: Source::Memory,
            binding: None,
        };
        pipe.read_exact(key.bytes.as_mut_slice())?;
        let mut source = [0u8; 1];
        pipe.read_exact(&mut source)?;
        key.source = Source::from_byte(source[0]).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "not a source"))?;
        let mut corpus = [0u8; KEY_BYTES];
        pipe.read_exact(&mut corpus)?;
        key.binding = Some(corpus);
        Ok(key)
    }

    /// A key a test makes up. Never for anything that's kept: only built for this crate's tests and with `test-seams`, which nothing shipped turns on.
    #[cfg(any(test, feature = "test-seams"))]
    pub fn for_tests(byte: u8) -> Self {
        Self {
            bytes: Box::new([byte; KEY_BYTES]),
            source: Source::Memory,
            binding: None,
        }
    }
}

/// The app the Find key is for: its team and bundle identifier, as its release is signed and its provisioning profile names them (`recto_update::TEAM_ID` and `BUNDLE_ID`, given by the app's start). Its access group in the keychain is its app ID, the two joined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct App {
    pub team: &'static str,
    pub bundle: &'static str,
}

impl App {
    /// The keychain access group the key is in: the app ID, `<team>.<bundle id>`, which the release's profile allows and its signature claims.
    #[must_use]
    pub fn access_group(self) -> String {
        format!("{}.{}", self.team, self.bundle)
    }
}

/// **The app's own key, and why it isn't the Keychain's when it isn't.** The Keychain is asked only by a copy that is the team's release, checked whole ([`keychain`]); any refusal, and every other copy, gets a key in memory, with the refusal's code. `None` only when the OS gives no randomness, and then nothing is indexed. The only way to the Keychain from outside this crate. Its one caller is `recto_control::find_keys::AppKey`, made only by the app's start; the indexer takes it on no main thread.
pub fn load_for_the_app(app: App) -> Option<(FindKey, Option<&'static str>)> {
    let refused = match refused_by(keychain::signed_as_a_release(app.team)) {
        None => match keychain::load_or_create(keychain::SERVICE, keychain::ACCOUNT, &app.access_group()) {
            // Said in `qa state` if the switch couldn't be set: nothing prompted, but nothing then guarded against it.
            Ok(key) => return Some((key, why_with_the_keychains_key(keychain::prompts_switched_off()))),
            Err(refused) => refused,
        },
        Some(refused) => refused,
    };
    FindKey::fresh(fallback_source(refused)).ok().map(|key| (key, Some(refused.code())))
}

/// What's said beside a key the Keychain gave: nothing, unless the process-wide switch couldn't be set, which doesn't stop the key being taken (the security read's F1).
fn why_with_the_keychains_key(switched_off: bool) -> Option<&'static str> {
    (!switched_off).then_some("keychain_prompts_not_switched_off")
}

/// What the release check's answer means for the Keychain: asked, refused as not the team's (a check made and failed), or refused as not to be had (a check that couldn't be made, which gives a key in memory for now).
fn refused_by(answer: keychain::Release) -> Option<keychain::Refused> {
    match answer {
        keychain::Release::Yes => None,
        keychain::Release::No => Some(keychain::Refused::NotTheTeam),
        keychain::Release::Unknown => Some(keychain::Refused::Unavailable),
    }
}

/// **Which key a run of the app goes on with when the Keychain gave none, by why** (the decision apart from the Keychain, so it's tested both ways): a copy that's not the team's release can never have the lasting key, so its key in memory may build over what that key sealed ([`Source::Memory`]); any other refusal means the lasting key may be there and is out of reach for now, so what it sealed is left as it is ([`Source::MemoryForNow`]).
fn fallback_source(refused: keychain::Refused) -> Source {
    if refused == keychain::Refused::NotTheTeam {
        Source::Memory
    } else {
        Source::MemoryForNow
    }
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

/// For a process that holds the key or pages' text: a crash leaves no image of its memory. Called by the indexer helper first thing, and by `recto_control::indexing::Indexer::new`, so by the app and by `recto` on a state with an index, before either can take the key. macOS writes none unless the limit's raised, and this lowers it for good whatever the shell set: the cost is that someone who raised `ulimit -c` to debug the app gets no core file from it. The crash report macOS writes (`ReportCrash`, an `.ips`) doesn't depend on the limit, so the app keeps those; it carries registers and the stack's addresses, not memory.
pub fn forbid_core_dumps() {
    #[cfg(target_os = "macos")]
    {
        let none = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // SAFETY: a plain call with a value on the stack.
        let _ = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &none) };
    }
}

/// The core limit now, soft and hard, for a test that [`forbid_core_dumps`] was called. `None` where it can't be read.
#[cfg(any(test, feature = "test-seams"))]
#[must_use]
pub fn core_limit() -> Option<(u64, u64)> {
    let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: a plain call with a value on the stack.
    (unsafe { libc::getrlimit(libc::RLIMIT_CORE, &mut limit) } == 0).then_some((limit.rlim_cur, limit.rlim_max))
}

impl Drop for FindKey {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(self.bytes.as_mut_slice());
    }
}

impl std::fmt::Debug for FindKey {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.binding.is_some() {
            write!(out, "FindKey({}, one corpus)", self.source.code())
        } else {
            write!(out, "FindKey({})", self.source.code())
        }
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

    /// Failure (the security read's F1): a release check that couldn't be made (Security.framework not had, this copy's code not found, a requirement that didn't compile) is taken for "not the team's release", whose key in memory may build over the person's lasting index; or one made and failed is taken for one not made. Each answer has its refusal, and only "not the team's" may replace.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_release_check_that_wasnt_made_never_builds_over_the_lasting_index() {
        use keychain::{Refused, Release};
        assert_eq!(refused_by(Release::Yes), None);
        assert_eq!(refused_by(Release::No), Some(Refused::NotTheTeam));
        assert_eq!(refused_by(Release::Unknown), Some(Refused::Unavailable));
        assert!(
            !fallback_source(Refused::Unavailable).may_replace_lasting(),
            "a check not made gave a key that builds over the lasting index"
        );
        assert!(fallback_source(Refused::NotTheTeam).may_replace_lasting());
        // The switch missing or refusing is said, and takes nothing away.
        assert_eq!(why_with_the_keychains_key(true), None);
        assert_eq!(why_with_the_keychains_key(false), Some("keychain_prompts_not_switched_off"));
    }

    /// Failure (the second review's item 4, 2026-11-15): the app's key after a Keychain refusal is the wrong kind either way. A copy that's not the team's release gets a key only for now, so it can never build over a corpus it can't read; or a release whose Keychain is locked, or whose item can't be read, gets one that may replace, so the person's index is thrown away for a moment's refusal.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_refusal_gives_the_key_its_reason_says() {
        use keychain::Refused;
        let table = [
            (Refused::NotTheTeam, Source::Memory),
            (Refused::Unavailable, Source::MemoryForNow),
            (Refused::Unreadable(-25293), Source::MemoryForNow),
            (Refused::Unreadable(-25308), Source::MemoryForNow),
            (Refused::NotAKey, Source::MemoryForNow),
            (Refused::NoRandomness, Source::MemoryForNow),
            (Refused::NotEntitled, Source::MemoryForNow),
        ];
        for (refused, source) in table {
            assert_eq!(fallback_source(refused), source, "{refused:?}");
            assert_eq!(fallback_source(refused).may_replace_lasting(), refused == Refused::NotTheTeam, "{refused:?}");
        }
    }

    /// Failure: a key can be printed, or two runs get the same key from nothing, or a key handed over arrives as something else.
    #[test]
    fn a_key_hides_itself_and_crosses_a_pipe_whole() {
        let key = FindKey::in_memory().unwrap();
        assert_eq!(format!("{key:?}"), "FindKey(memory)");
        assert_ne!(key.bytes(), FindKey::in_memory().unwrap().bytes(), "two fresh keys are the same");
        // The session's key never crosses a pipe (S1): only one bound to a corpus does.
        assert!(key.write_to(&mut Vec::new()).is_err(), "a session's key was written to a helper");
        let bound = FindKey::bound(&[5u8; KEY_BYTES], Source::Keychain, [6u8; KEY_BYTES]);
        assert_eq!(format!("{bound:?}"), "FindKey(keychain, one corpus)");
        let mut pipe = Vec::new();
        bound.write_to(&mut pipe).unwrap();
        pipe.extend_from_slice(b"{\"job\":1}\n");
        let mut reader = pipe.as_slice();
        let handed = FindKey::read_from(&mut reader).unwrap();
        assert_eq!((handed.bytes(), handed.binding()), (bound.bytes(), Some(&[6u8; KEY_BYTES])));
        assert_eq!(
            handed.source(),
            bound.source(),
            "a handed key loses its session's source, so what the helper seals says it doesn't last"
        );
        assert_eq!(reader, b"{\"job\":1}\n", "the job after the key is left for the job's reader");
        for source in [Source::Memory, Source::MemoryForNow, Source::Keychain, Source::StateFile] {
            assert_eq!(Source::from_byte(source.byte()), Some(source));
        }
        let mut frame = Vec::new();
        bound.write_to(&mut frame).unwrap();
        let mut old = frame.clone();
        old[0] = 1;
        assert!(FindKey::read_from(&mut old.as_slice()).is_err(), "a frame of another version was taken");
        let mut no_source = frame.clone();
        no_source[1 + KEY_BYTES] = 9;
        assert!(FindKey::read_from(&mut no_source.as_slice()).is_err(), "a source that isn't one was taken");
        assert!(FindKey::read_from(&mut &frame[..frame.len() - 1]).is_err(), "a frame cut short was taken");
        assert!(
            FindKey::read_from(&mut &[1u8; 33][..]).is_err(),
            "the old frame, a key and its source, was taken"
        );
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
