//! **An agent's folder lives only for the session that asked for it** (the coordinator's decision on the Find text's design, 2026-11-15): it's the document's text in plain Markdown, for an agent's own `rg`, so it doesn't stay once that session ends. At the end of a session each folder it made is moved aside in one step (`agent` → `agent.gone`, no walk of its files, so a quit doesn't wait), and the next session's first sweep removes what's aside and any folder older than that session. A mark beside the corpora says there may be some, so a start needs one look at a file to know there's nothing to do.

use super::{CorpusKey, CorpusPaths};
use recto_platform::local;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Beside the corpora: an agent's folder was made since the last sweep. Not named as a key, so nothing takes it for a corpus.
const AGENT_MARK: &str = "agent-folders";

fn set_aside(paths: &CorpusPaths) -> PathBuf {
    paths.agent.with_file_name("agent.gone")
}

/// Notes that an agent's folder is about to be made, before it is, so a start knows to look.
pub fn note_agent_folder(root: &Path) {
    let _ = local::write_private_file_unsynced(&root.join(AGENT_MARK), b"");
}

/// Whether a start should sweep agents' folders.
pub fn agent_folders_may_be_there(root: &Path) -> bool {
    root.join(AGENT_MARK).exists()
}

/// Moves a session's folder aside, in one step: the end of the session that made it.
pub fn retire_agent_folder(root: &Path, key: CorpusKey) {
    let paths = CorpusPaths::new(root, key);
    let aside = set_aside(&paths);
    if fs::symlink_metadata(&aside).is_ok() {
        let _ = fs::remove_dir_all(&aside);
    }
    let _ = fs::rename(&paths.agent, &aside);
}

/// Removes every agents' folder set aside, and every one older than `session_began`, but those in `keep`: what this session has asked for. Only folders named as keys are looked at, and nothing a link leads to. Returns how many went. The mark goes once there's nothing left.
pub fn sweep_agent_folders(root: &Path, session_began: SystemTime, keep: &[CorpusKey]) -> u32 {
    let Ok(entries) = fs::read_dir(root) else { return 0 };
    let (mut removed, mut left) = (0, 0);
    for entry in entries.flatten() {
        let Some(key) = entry.file_name().to_str().and_then(CorpusKey::from_hex) else {
            continue;
        };
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let paths = CorpusPaths::new(root, key);
        let aside = set_aside(&paths);
        if fs::symlink_metadata(&aside).is_ok_and(|meta| meta.is_dir()) {
            match fs::remove_dir_all(&aside) {
                Ok(()) => removed += 1,
                Err(_) => left += 1,
            }
        }
        let Ok(meta) = fs::symlink_metadata(&paths.agent) else { continue };
        if !meta.is_dir() {
            continue;
        }
        let made = fs::symlink_metadata(&paths.manifest)
            .and_then(|meta| meta.modified())
            .or_else(|_| meta.modified());
        if keep.contains(&key) || made.is_ok_and(|made| made >= session_began) {
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
