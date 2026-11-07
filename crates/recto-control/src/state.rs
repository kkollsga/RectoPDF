//! Where the session keeps its files. One folder per user, private to them. Tests and QA point it elsewhere with [`STATE_ENV`], and that override refuses the real home, so a test can't touch the
//! user's real level, log or session however it's run.

use recto_agent::{AgentError, exit};
use recto_platform::local;
use std::path::{Component, Path, PathBuf};

/// Points the state folder somewhere else, for tests and QA. It can't raise a level: a new folder starts at Off.
pub const STATE_ENV: &str = "RECTO_STATE_DIR";

/// Longest socket path we'll use directly. `bind` allows a little more; this leaves room for nothing to go wrong.
const SOCKET_PATH_LIMIT: usize = 100;

/// Every file the session uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatePaths {
    pub root: PathBuf,
    /// The private folder the socket and lock live in.
    pub run_dir: PathBuf,
    pub socket: PathBuf,
    pub lock: PathBuf,
    pub level: PathBuf,
    pub log: PathBuf,
    pub old_log: PathBuf,
    /// Everything that doesn't change anything, so it can't push out what does.
    pub other_log: PathBuf,
    pub old_other_log: PathBuf,
    /// Where a background session's own messages go.
    pub server_messages: PathBuf,
    /// Set through [`STATE_ENV`], not the user's real folder.
    pub overridden: bool,
}

impl StatePaths {
    /// The folder this run should use: the override if there is one, after refusing the real home, or else the user's own.
    pub fn from_env() -> Result<Self, AgentError> {
        match std::env::var_os(STATE_ENV) {
            Some(path) => Self::overridden(Path::new(&path)),
            None => Ok(Self::at(default_root()?, false)),
        }
    }

    /// A state folder other than the user's own, after checking it isn't their home in disguise. Nothing is created until a checked path comes back.
    pub fn overridden(path: &Path) -> Result<Self, AgentError> {
        let wanted = resolved(path).ok_or_else(|| refused(path))?;
        let home = local::real_home().and_then(|home| home.canonicalize().ok()).ok_or_else(|| refused(path))?;
        // From the OS, because `$TMPDIR` can say anything, including somewhere in the home.
        let temp = local::system_temp_dir().and_then(|temp| temp.canonicalize().ok());
        if refuses_real_home(&wanted, &home, temp.as_deref()) {
            return Err(refused(path));
        }
        Ok(Self::at(wanted, true))
    }

    fn at(root: PathBuf, overridden: bool) -> Self {
        let run_dir = root.join("run");
        let direct = run_dir.join("recto.sock");
        // The state folder can be too long for a socket's name, and the limit is the kernel's. Then the socket lives in a short folder of its own, named for the state folder.
        let (run_dir, socket) = if direct.as_os_str().len() <= SOCKET_PATH_LIMIT {
            (run_dir, direct)
        } else {
            let short = PathBuf::from(format!("/tmp/recto-{}", local::current_uid()));
            let name = format!("{:016x}.sock", fingerprint(root.as_os_str().as_encoded_bytes()));
            (short.clone(), short.join(name))
        };
        Self {
            lock: root.join("run").join("server.lock"),
            level: root.join("level"),
            log: root.join("activity.jsonl"),
            old_log: root.join("activity.1.jsonl"),
            other_log: root.join("activity-other.jsonl"),
            old_other_log: root.join("activity-other.1.jsonl"),
            server_messages: root.join("server-messages.txt"),
            root,
            run_dir,
            socket,
            overridden,
        }
    }

    /// Makes the folders, private to this user. A folder that's already there and isn't private is refused, never fixed: it may be a trap.
    pub fn prepare(&self) -> Result<(), AgentError> {
        let failed = |path: &Path| AgentError::new("state_refused", format!("The state folder {} isn't private to this user, or can't be made.", display(path)), exit::FAILED).then("Choose another folder, or check who owns this one.");
        local::ensure_private_dir(&self.root).map_err(|_| failed(&self.root))?;
        local::ensure_private_dir(&self.root.join("run")).map_err(|_| failed(&self.root))?;
        local::ensure_private_dir(&self.run_dir).map_err(|_| failed(&self.run_dir))?;
        Ok(())
    }
}

fn display(path: &Path) -> String {
    recto_core::clean_line(&path.display().to_string())
}

fn refused(path: &Path) -> AgentError {
    AgentError::new("state_refused", format!("{STATE_ENV} can't be {}: it's the real home, or inside it.", display(path)), exit::PERMISSION).then("Point it at a new folder in a temp folder.")
}

/// The user's own folder. Under the real home, whatever `$HOME` says.
fn default_root() -> Result<PathBuf, AgentError> {
    local::real_home().map(|home| default_root_in(&home)).ok_or_else(|| AgentError::new("state_refused", "This user's home folder couldn't be found.", exit::FAILED))
}

fn default_root_in(home: &Path) -> PathBuf {
    home.join("Library").join("Application Support").join("RectoPDF").join("agent")
}

/// The rule. Refused: the home itself, any folder holding it, and anything inside it, unless that's inside the system's temp folder. A temp folder that holds the home proves nothing, so it doesn't count.
pub fn refuses_real_home(wanted: &Path, home: &Path, temp: Option<&Path>) -> bool {
    if wanted == home || home.starts_with(wanted) {
        return true;
    }
    if !wanted.starts_with(home) {
        return false;
    }
    if wanted.starts_with(default_root_in(home)) {
        return true;
    }
    !temp.is_some_and(|temp| !home.starts_with(temp) && wanted.starts_with(temp))
}

/// Whether a path, wherever links and `..` really take it, is in this user's real home.
pub fn is_in_real_home(path: &Path) -> bool {
    let Some(home) = local::real_home().and_then(|home| home.canonicalize().ok()) else { return true };
    resolved(path).is_none_or(|real| real.starts_with(&home) || is_inside(&real, &home))
}

/// Whether any folder up from a path is this one by what the filesystem says it is, and not by its name. `canonicalize` doesn't see through a firmlink, so `/System/Volumes/Data/...` is the same folder as the path without it, under a name that doesn't start the same.
#[cfg(unix)]
fn is_inside(path: &Path, folder: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(wanted) = std::fs::metadata(folder) else { return true };
    path.ancestors().filter_map(|up| std::fs::metadata(up).ok()).any(|found| found.dev() == wanted.dev() && found.ino() == wanted.ino())
}

#[cfg(not(unix))]
fn is_inside(_path: &Path, _folder: &Path) -> bool {
    false
}

/// A path's real location, following links as far as it exists. Without this a link to the home would pass.
fn resolved(path: &Path) -> Option<PathBuf> {
    let absolute = std::path::absolute(path).ok()?;
    let mut existing = absolute.as_path();
    let mut missing = Vec::new();
    loop {
        if let Ok(real) = existing.canonicalize() {
            let mut out = real;
            for part in missing.iter().rev() {
                out.push(part);
            }
            return Some(out);
        }
        // A link that points nowhere has no safe meaning here.
        if existing.symlink_metadata().is_ok() {
            return None;
        }
        let name = existing.components().next_back()?;
        if !matches!(name, Component::Normal(_)) {
            return None;
        }
        missing.push(name.as_os_str().to_os_string());
        existing = existing.parent()?;
    }
}

fn fingerprint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a test or QA run points the state folder at the user's real home, or somewhere inside it, and writes over their real level or log.
    #[test]
    fn the_override_refuses_the_real_home() {
        let home = Path::new("/Users/me");
        let temp = Some(Path::new("/private/tmp"));
        assert!(refuses_real_home(home, home, temp));
        assert!(refuses_real_home(Path::new("/Users"), home, temp), "a folder holding the home");
        assert!(refuses_real_home(Path::new("/"), home, temp));
        assert!(refuses_real_home(Path::new("/Users/me/Documents/qa"), home, temp), "inside it");
        assert!(refuses_real_home(&default_root_in(home), home, temp), "the real state folder");
        assert!(refuses_real_home(&default_root_in(home).join("sub"), home, temp));
        assert!(!refuses_real_home(Path::new("/private/tmp/qa"), home, temp));
        assert!(!refuses_real_home(Path::new("/Volumes/Other/qa"), home, temp));
        assert!(!refuses_real_home(Path::new("/Users/other/qa"), home, temp));
        // A temp folder inside the home is fine, but one holding the home proves nothing.
        assert!(!refuses_real_home(Path::new("/Users/me/tmp/qa"), home, Some(Path::new("/Users/me/tmp"))));
        assert!(refuses_real_home(Path::new("/Users/me/qa"), home, Some(home)));
        assert!(refuses_real_home(Path::new("/Users/me/qa"), home, Some(Path::new("/"))));
        assert!(refuses_real_home(Path::new("/Users/me/qa"), home, None));
    }

    /// Failure: a link to the real home passes the check, because the path as written doesn't mention it.
    #[test]
    fn a_link_to_the_home_is_seen_through() {
        let home = local::real_home().unwrap().canonicalize().unwrap();
        let link = std::env::temp_dir().canonicalize().unwrap().join(format!("recto-home-link-{}", std::process::id()));
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&home, &link).unwrap();
        let error = StatePaths::overridden(&link).unwrap_err();
        assert_eq!(error.code, "state_refused");
        assert!(StatePaths::overridden(&home).is_err());
        assert!(StatePaths::overridden(&home.join("no-such-folder-recto/deeper")).is_err());
        std::fs::remove_file(&link).unwrap();
    }

    /// Failure: `$TMPDIR` set inside the home makes that count as a temp folder, so a test state there passes. The temp folder comes from the OS.
    #[test]
    fn tmpdir_cant_vouch_for_a_folder_in_the_home() {
        let home = local::real_home().unwrap().canonicalize().unwrap();
        let temp = local::system_temp_dir().unwrap().canonicalize().unwrap();
        assert!(!temp.starts_with(&home), "this Mac's own temp folder is outside the home");
        // SAFETY of the rule rather than of `$TMPDIR`: what's checked is only ever the OS's answer.
        assert!(refuses_real_home(&home.join("somewhere"), &home, Some(&temp)));
    }

    /// Failure: a path that climbs out of somewhere else and back into the home, or a link into it, reads as being outside.
    #[test]
    fn the_real_location_decides_what_is_in_the_home() {
        let home = local::real_home().unwrap().canonicalize().unwrap();
        assert!(is_in_real_home(&home.join("no-such-folder-recto/skill")));
        let climbing = PathBuf::from(format!("/usr/..{}/no-such-folder-recto", home.display()));
        assert!(is_in_real_home(&climbing), "{climbing:?}");
        let link = std::env::temp_dir().canonicalize().unwrap().join(format!("recto-in-home-{}", std::process::id()));
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&home, &link).unwrap();
        assert!(is_in_real_home(&link.join("no-such-folder-recto")));
        std::fs::remove_file(&link).unwrap();
        assert!(!is_in_real_home(&std::env::temp_dir().canonicalize().unwrap().join("skill")));
    }

    /// Failure: a long state folder makes a socket path the kernel refuses, which is what happens on a real home on a long path. The owner's own is close to the limit.
    #[test]
    fn a_long_state_folder_still_gets_a_short_socket() {
        let long = std::env::temp_dir().canonicalize().unwrap().join("x".repeat(90)).join("state");
        let paths = StatePaths::overridden(&long).unwrap();
        assert!(paths.socket.as_os_str().len() <= local::MAX_SOCKET_PATH, "{}", paths.socket.display());
        assert_ne!(paths.run_dir, long.join("run"));
        let other = StatePaths::overridden(&long.with_file_name("state2")).unwrap();
        assert_ne!(paths.socket, other.socket, "two state folders must not share a session");
        let short = StatePaths::overridden(&std::env::temp_dir().join("s")).unwrap();
        assert!(short.socket.ends_with("run/recto.sock"));
    }

    /// Failure: the state folder is made open to others, or one someone else made is used as it is.
    #[test]
    fn prepare_makes_private_folders_and_refuses_open_ones() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("recto-prepare-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = StatePaths::overridden(&root).unwrap();
        assert!(!root.exists(), "checking a path must not create it");
        paths.prepare().unwrap();
        assert_eq!(std::fs::metadata(&paths.root).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(&paths.run_dir).unwrap().permissions().mode() & 0o777, 0o700);
        std::fs::set_permissions(&paths.run_dir, std::fs::Permissions::from_mode(0o775)).unwrap();
        assert_eq!(paths.prepare().unwrap_err().code, "state_refused");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure: a firmlink's other name for the home folder gets past a check by name, so a QA shot can land in the real home. Run for real on this Mac's own firmlink: the path is only tried if it's there and it is the home, and the home's own path is always tried.
    #[test]
    #[cfg(target_os = "macos")]
    fn the_real_home_is_known_by_any_name() {
        use std::os::unix::fs::MetadataExt;
        let home = local::real_home().unwrap().canonicalize().unwrap();
        assert!(is_in_real_home(&home.join("x.png")));
        assert!(is_in_real_home(&home.join("a/folder/that/isnt/there/x.png")));
        let other_name = PathBuf::from("/System/Volumes/Data").join(home.strip_prefix("/").unwrap());
        if let (Ok(there), Ok(here)) = (std::fs::metadata(&other_name), std::fs::metadata(&home))
            && there.dev() == here.dev()
            && there.ino() == here.ino()
        {
            assert!(is_in_real_home(&other_name.join("x.png")), "{other_name:?}");
            assert!(is_in_real_home(&other_name.join("a/folder/that/isnt/there/x.png")));
        }
        assert!(!is_in_real_home(&std::env::temp_dir().join("x.png")) || std::env::temp_dir().canonicalize().unwrap().starts_with(&home));
    }
}
