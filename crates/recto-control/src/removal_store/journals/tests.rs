//! The adapter lists real folders in a temp root and places by headers and names; the plan made from it is what a person would be shown.

use super::*;
use recto_core::removal::{DESIGN_LIMITS, Disposition, Effect, FileVersion, Kinds, LEDGER_LIMITS, Ledger, Limits, Link, Plan, Request, Surveyed, lineage};
use recto_session::record::{JournalHeader, journal_line, journal_start};
use recto_testpdf::TempDir;
use std::num::NonZeroU16;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

const MINE: &str = "/docs/mine.pdf";
const THEIRS: &str = "/docs/theirs.pdf";
const FIVE: [Store; 5] = [Store::Journal, Store::JournalVariant, Store::PageIds, Store::SleepFile, Store::KeptPages];

fn version(inode: u64) -> FileVersion {
    FileVersion {
        volume: Some([3; 16]),
        inode,
        size: 10,
        mtime_ns: 1,
    }
}

fn stamp(inode: u64) -> FileStamp {
    FileStamp {
        size: 10,
        mtime_ns: 1,
        device: 0,
        inode,
    }
}

/// The 16 digits the names of version `inode` carry.
fn k(inode: u64) -> String {
    key_of(inode, 10, 1)
}

/// This document at `MINE` (inode 7), and another at `THEIRS` (inode 8), from the beginning of time.
fn ledger() -> Ledger {
    let at = |inode, path: &str| Link::At {
        version: version(inode),
        path: PathBuf::from(path),
    };
    Ledger {
        links: vec![at(7, MINE), at(8, THEIRS)],
        began_ns: 0,
        gap: None,
        stopped: None,
        good_len: 0,
        frames: 0,
        overflowed: false,
    }
}

fn limits(max: u32) -> Limits {
    Limits {
        max_artifacts: max,
        max_batch: NonZeroU16::new(4).unwrap(),
    }
}

/// A root with both sessions' journals folders.
fn root() -> TempDir {
    let dir = TempDir::new("recto-journals-");
    for flavor in FLAVORS {
        std::fs::create_dir_all(dir.join(flavor).join("journals")).unwrap();
    }
    dir
}

fn folder(root: &Path) -> PathBuf {
    root.join("session").join("journals")
}

fn windowless(root: &Path) -> PathBuf {
    root.join("session-windowless").join("journals")
}

/// A survey of `root` with every store but the five marked complete, so the plan decides on the journals folders alone.
fn survey_with(root: &Path, ledger: &Ledger, current: FileVersion, mode: TargetMode, max: u32, caps: &Caps) -> Result<Surveyed, Refusal> {
    let lineage = lineage(ledger, current, Path::new(MINE), &LEDGER_LIMITS).unwrap();
    let mut survey = Survey::new(&limits(max));
    for store in Store::ALL.into_iter().filter(|s| s.enumerated() && !FIVE.contains(s)) {
        survey.mark_complete(store);
    }
    survey.history(&lineage);
    survey_journals_with(root, &lineage, mode, &mut survey, caps)?;
    survey.finish()
}

fn survey(root: &Path, mode: TargetMode) -> Result<Surveyed, Refusal> {
    survey_with(root, &ledger(), version(7), mode, 64, &Caps::default())
}

fn plan_of(s: &Surveyed, mode: TargetMode) -> Result<Plan, Refusal> {
    let request = Request::new(Kinds::from_bits(Kinds::METADATA).unwrap(), mode, false);
    recto_core::removal::plan(&request, &s.inventory(), &DESIGN_LIMITS)
}

fn plan(root: &Path, mode: TargetMode) -> Result<Plan, Refusal> {
    plan_of(&survey(root, mode)?, mode)
}

fn refusal(root: &Path, mode: TargetMode) -> Refusal {
    plan(root, mode).unwrap_err()
}

fn purged(p: &Plan, store: Store) -> usize {
    p.steps()
        .iter()
        .filter(|s| s.effect() == Effect::Purge(store))
        .map(|s| p.artifacts(s).len())
        .sum()
}

/// A journal's bytes: its first line, for a tab with this file and path.
fn journal(path: &str, inode: u64) -> Vec<u8> {
    journal_start(&JournalHeader::new(1, Path::new(path), Some(stamp(inode)), 3, 0), None)
}

fn put(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn mkfifo(path: &Path) {
    assert!(std::process::Command::new("mkfifo").arg(path).status().unwrap().success());
}

/// Runs `work` on a thread and fails if it takes over 10 s, so a pipe waited on fails the test and doesn't hang the run.
fn bounded<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    receiver.recv_timeout(Duration::from_secs(10)).expect("the survey waited on something")
}

/// Failure: a link is followed (a symlink to a file that would place as the target's is taken for the target's own and purged, or read for its
/// header), or a pipe is waited on (the survey hangs on a pipe with no writer). Each of the five kinds of name, as a pipe and as a link.
#[test]
fn a_pipe_or_a_link_at_any_name_is_unknown_and_never_followed() {
    let key = k(7);
    let cases = [
        (format!("journal-{key}.log"), Store::Journal),
        (format!("journal-{key}.log.unreadable"), Store::JournalVariant),
        (format!("pages-{key}.ids"), Store::PageIds),
        (format!("sleep-{key}.bin"), Store::SleepFile),
        (format!("kept-{key}.keptpages"), Store::KeptPages),
    ];
    for (name, store) in cases {
        let dir = root();
        mkfifo(&folder(&dir).join(&name));
        let path = dir.path().to_path_buf();
        assert_eq!(
            bounded(move || refusal(&path, TargetMode::Save)),
            Refusal::Incomplete(store),
            "a pipe at {name}"
        );

        let dir = root();
        let outside = TempDir::new("recto-journals-out-");
        // What a link here would place as the target's, if followed.
        let precious = put(&outside, "precious", &journal(MINE, 7));
        std::os::unix::fs::symlink(&precious, folder(&dir).join(&name)).unwrap();
        let path = dir.path().to_path_buf();
        assert_eq!(
            bounded(move || refusal(&path, TargetMode::Save)),
            Refusal::Incomplete(store),
            "a link at {name}"
        );
        assert_eq!(std::fs::read(&precious).unwrap(), journal(MINE, 7));
    }
}
