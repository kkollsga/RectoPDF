//! **An agent's folder lives only for the session that asked for it** (the coordinator's decision on the Find text's design, 2026-11-15): it's the document's text in plain Markdown, for an agent's own `rg`, so it doesn't stay once that session ends.
//!
//! - **Whose it is:** the session that asks holds a lock on a file in the folder, `agent/.session`, for as long as it lives ([`AgentClaim`]); the kernel lets that lock go however the session ends, a crash included. A folder whose lock anyone can take is a dead session's.
//! - **At the end of a session** each folder it holds is moved aside in one rename, to a name of its own (`agent.gone.<pid>.<random>`), no walk of its files and nothing removed, so a quit doesn't wait.
//! - **The sweep,** the first time a session uses an index, on the indexer's thread and under no lock a search or a quit waits on: removes what's set aside, at most [`MAX_REMOVED_PER_SWEEP`] folders a pass (each at most a corpus's pages, 200,000 files), and every agent's folder whose session is gone. A folder another live session holds is left. A page file a kill left half-made (`agent/.page.tmp`) goes with its folder, or on its own from a folder of this session's when no one's writing it.
//! - **Nothing outside the corpora:** every folder on the way to an agent's folder is looked at without following a link (as reading a corpus does), and only folders named as keys, before anything is renamed or removed. A link anywhere on the way is left as it is.
//!
//! A mark beside the corpora says there may be some to sweep, so a start needs one look at a file to know there's nothing to do.

use super::{CorpusKey, CorpusPaths, key_folders};
use recto_platform::local;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

/// Beside the corpora: an agent's folder was made since the last sweep. Not named as a key, so nothing takes it for a corpus.
const AGENT_MARK: &str = "agent-folders";
/// In an agent's folder: what its session holds a lock on.
const SESSION_FILE: &str = ".session";
/// How a folder set aside is named, before its own suffix.
const ASIDE: &str = "agent.gone";
/// Most folders one sweep removes. The rest wait for the next session's.
pub const MAX_REMOVED_PER_SWEEP: usize = 64;

/// A session's hold on an agent's folder: a lock on its `.session` file, let go when this is dropped or the process ends.
#[derive(Debug)]
pub struct AgentClaim(File);

impl AgentClaim {
    /// Lets the session's hold go and moves nothing: unlocked, then closed (a lock belongs to the open description, so closing alone could leave it held while a copy of the descriptor lives; `record_store/lock.rs` says why). The folder stays where it is, and what looks at its `.session` finds it free.
    pub fn release(self) {
        let _ = self.0.unlock();
    }
}

/// The folders on the way to a corpus's agent folder, if each is really a private folder of this user's, with no link: the only state in which anything here is renamed or removed.
fn checked(root: &Path, key: CorpusKey) -> Option<CorpusPaths> {
    let paths = CorpusPaths::new(root, key);
    paths.folders_are_private(root).then_some(paths)
}

fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
}

/// Notes that an agent's folder is about to be made, before it is, so a start knows to look.
pub fn note_agent_folder(root: &Path) {
    let _ = local::write_private_file_unsynced(&root.join(AGENT_MARK), b"");
}

/// Whether a start should sweep agents' folders.
pub fn agent_folders_may_be_there(root: &Path) -> bool {
    root.join(AGENT_MARK).exists()
}

/// This session's hold on the corpus's agent folder, made if it isn't there. `None` if the folders on the way aren't all private ones, or another session that's still running holds it: it's that one's to end.
pub fn claim_agent_folder(root: &Path, key: CorpusKey) -> Option<AgentClaim> {
    let paths = checked(root, key)?;
    local::ensure_private_dir(&paths.agent).ok()?;
    let file = local::append_private_file(&paths.agent.join(SESSION_FILE)).ok()?;
    file.try_lock().ok()?;
    Some(AgentClaim(file))
}

/// Moves a session's folder aside, in one rename to a name of its own, and lets the session's hold go: the end of the session that holds it. Only with its claim: a session that got none, because another live session holds the folder, has nothing to set aside (the second review's item 1, 2026-11-15).
pub fn retire_agent_folder(root: &Path, key: CorpusKey, claim: AgentClaim) {
    if let Some(paths) = checked(root, key)
        && is_real_dir(&paths.agent)
    {
        let aside = paths
            .agent
            .with_file_name(format!("{ASIDE}.{}.{:016x}", std::process::id(), local::random_u64()));
        let _ = fs::rename(&paths.agent, aside);
    }
    drop(claim);
}

/// Whether some live session holds this folder: its `.session` lock can't be taken.
pub(super) fn held_by_a_session(agent: &Path) -> bool {
    local::open_private_file(&agent.join(SESSION_FILE)).is_ok_and(|file| file.try_lock().is_err())
}

/// Takes away what an agent's folder says, its pages and manifest and any half-made page, and keeps whose it is: its `.session`, on which its session's lock is, stays, so the folder is still that session's to end (the second review's item 3, 2026-11-15). A folder with no `.session` is no session's, and goes once it's empty.
pub(super) fn empty_agent_folder(paths: &CorpusPaths) {
    let _ = fs::remove_dir_all(&paths.pages);
    let _ = fs::remove_file(&paths.manifest);
    let _ = fs::remove_file(&paths.temp_page);
    let _ = fs::remove_dir(&paths.agent);
}

/// Makes the pages' folder inside an agent's folder that's there, never the agent's folder itself: so a build that's still running when its session sets the folder aside can't make one again in its place.
pub(super) fn make_pages_in_held_folder(paths: &CorpusPaths) -> std::io::Result<()> {
    #[cfg(unix)]
    let made = std::os::unix::fs::DirBuilderExt::mode(&mut fs::DirBuilder::new(), 0o700).create(&paths.pages);
    #[cfg(not(unix))]
    let made = fs::create_dir(&paths.pages);
    match made {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => local::check_private_dir(&paths.pages),
        Err(error) => Err(error),
    }
}

/// The folders set aside in a corpus's folder.
fn set_aside(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name().to_str().is_some_and(|name| name.starts_with(ASIDE)))
        .map(|entry| entry.path())
        .filter(|path| is_real_dir(path))
        .collect()
}

/// **The sweep:** removes what's set aside and every agent's folder no live session holds, but those in `keep`, this session's own, from which only a half-made page file goes when no one's writing it. At most [`MAX_REMOVED_PER_SWEEP`] folders. Returns how many went. The mark goes once there's nothing left.
pub fn sweep_agent_folders(root: &Path, keep: &[CorpusKey]) -> u32 {
    let (mut removed, mut left) = (0u32, 0u32);
    for (key, _) in key_folders(root) {
        let Some(paths) = checked(root, key) else { continue };
        for aside in set_aside(&paths.dir) {
            if removed as usize >= MAX_REMOVED_PER_SWEEP {
                left += 1;
            } else if fs::remove_dir_all(&aside).is_ok() {
                removed += 1;
            } else {
                left += 1;
            }
        }
        if !is_real_dir(&paths.agent) {
            continue;
        }
        if keep.contains(&key) {
            left += 1;
            remove_a_half_made_page(&paths);
            continue;
        }
        if held_by_a_session(&paths.agent) || removed as usize >= MAX_REMOVED_PER_SWEEP {
            left += 1;
            continue;
        }
        match fs::remove_dir_all(&paths.agent) {
            Ok(()) => removed += 1,
            Err(_) => left += 1,
        }
    }
    if left == 0 {
        let _ = fs::remove_file(root.join(AGENT_MARK));
    }
    removed
}

/// A page file a kill left half-made in a folder of this session's, if nobody's writing one now (the corpus's lock is free).
fn remove_a_half_made_page(paths: &CorpusPaths) {
    if fs::symlink_metadata(&paths.temp_page).is_err() {
        return;
    }
    if let Ok(lock) = local::append_private_file(&paths.lock)
        && lock.try_lock().is_ok()
    {
        let _ = fs::remove_file(&paths.temp_page);
    }
}
