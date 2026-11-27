//! Hostile cache folders (`recto_control::removal_store::survey_caches`): whatever bytes a version note holds and whatever names the corpus root, `by-file` and
//! the contents folder hold, the survey never panics, answers only `Ok` or `TooLarge`, finds only the four stores it owns, never calls anything `Unrelated`, and
//! reads a note as the app's own format (`Format::Current`) exactly when it is 1 to 32 distinct keys one a line with no stray byte. A note named for the target's
//! path is the target's, so what its content says is what the plan would be asked to rewrite.
//!
//! Input: byte 0 is how many bytes of name follow the next byte (0 to 80), byte 1 what the name is in the corpus root (0 a file, 1 a folder, 2 a link, 3 a hard link
//! to the note); then the name (`/` and NUL made `_`, any other byte kept, so a name that is not text is made), and the rest is the note's content.

use recto_control::removal_store::{CacheTarget, survey_caches};
use recto_core::removal::{FileVersion, Surveyed};
use recto_core::removal::{Format, LEDGER_LIMITS, Ledger, Limits, Link, Owner, Refusal, Store, Survey, TargetMode, lineage};
use recto_testpdf::TempDir;
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::num::NonZeroU16;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

const OURS: [Store; 4] = [Store::CorpusText, Store::CorpusAgentPages, Store::CorpusAliases, Store::GeneratedContents];

fn key(byte: u8) -> String {
    format!("{byte:02x}").repeat(32)
}

/// Valid inputs that reach each shape: a good note, 32 keys, a carriage return, an uppercase key's folder, a hard link, a name that isn't text.
pub fn seeds() -> Vec<Vec<u8>> {
    let with = |name: &[u8], kind: u8, content: &[u8]| {
        let mut v = vec![name.len() as u8, kind];
        v.extend(name);
        v.extend(content);
        v
    };
    let many = |n: u8| (0..n).map(key).collect::<Vec<_>>().join("\n");
    vec![
        with(b"", 0, key(1).as_bytes()),
        with(b"stray", 0, many(32).as_bytes()),
        with(b"stray", 1, many(33).as_bytes()),
        with(key(0xab).to_uppercase().as_bytes(), 1, format!("{}\r\n{}", key(1), key(2)).as_bytes()),
        with(key(0xcd).as_bytes(), 1, format!("{}\n", key(1)).as_bytes()),
        with(b"sealed", 3, format!("{}\n{}", key(3), key(3)).as_bytes()),
        with(&[0xff, 0xfe, b'x'], 2, b""),
        with(b"agent-folders", 0, &[0xff; 70]),
    ]
}

/// The independent statement of the format the app writes: what `Format::Current` has to mean.
fn is_the_apps_format(content: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(content) else { return false };
    if content.len() > 32 * 65 || text.is_empty() {
        return false;
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let hex = |line: &&str| line.len() == 64 && line.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let mut distinct = lines.clone();
    distinct.sort_unstable();
    distinct.dedup();
    lines.len() <= 32 && lines.iter().all(hex) && distinct.len() == lines.len()
}

fn alias_named_for(path: &Path) -> String {
    let mut hash = Sha256::new();
    hash.update(b"path\0");
    hash.update(path.as_os_str().as_encoded_bytes());
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The check libFuzzer, the smoke run and the regression tests share.
pub fn check(data: &[u8]) {
    let Some((&length, rest)) = data.split_first() else { return };
    let Some((&kind, rest)) = rest.split_first() else { return };
    let length = usize::from(length % 81).min(rest.len());
    let (name, content) = rest.split_at(length);
    let name: Vec<u8> = name.iter().map(|&b| if b == b'/' || b == 0 { b'_' } else { b }).collect();

    let dir = TempDir::new("recto-fuzz-caches-");
    let (corpus, contents) = (dir.join("corpus.noindex"), dir.join("contents.noindex"));
    let mine: PathBuf = dir.join("mine.pdf");
    let by_file = corpus.join("by-file");
    let _ = std::fs::create_dir_all(&by_file);
    let _ = std::fs::create_dir_all(&contents);
    let note = by_file.join(alias_named_for(&mine));
    let _ = std::fs::write(&note, content);
    if !name.is_empty() {
        #[cfg(unix)]
        let name = OsString::from_vec(name);
        #[cfg(not(unix))]
        let name = OsString::from(String::from_utf8_lossy(&name).into_owned());
        let in_root = corpus.join(&name);
        let _ = match kind % 4 {
            0 => std::fs::write(&in_root, content),
            1 => std::fs::create_dir(&in_root),
            #[cfg(unix)]
            2 => std::os::unix::fs::symlink("/nonexistent", &in_root),
            #[cfg(not(unix))]
            2 => std::fs::write(&in_root, content),
            _ => std::fs::hard_link(&note, &in_root),
        };
        let _ = std::fs::write(by_file.join(&name), content);
        let _ = std::fs::write(contents.join(&name), content);
    }

    let ledger = Ledger {
        links: vec![Link::At {
            version: FileVersion {
                volume: None,
                inode: 7,
                size: 10,
                mtime_ns: 1,
            },
            path: mine.clone(),
        }],
        destinations: Vec::new(),
        path_bytes: 0,
        began_ns: 0,
        gap: None,
        stopped: None,
        good_len: 0,
        frames: 0,
        overflowed: false,
    };
    let current = FileVersion {
        volume: None,
        inode: 7,
        size: 10,
        mtime_ns: 1,
    };
    let lineage = lineage(&ledger, current, &mine, &LEDGER_LIMITS).expect("a one-link ledger");
    let mut survey = Survey::new(&Limits {
        max_artifacts: 512,
        max_batch: NonZeroU16::new(4).expect("not zero"),
    });
    let target = CacheTarget {
        key: None,
        device_inode: Some((1, 7)),
    };
    match survey_caches(&corpus, &contents, &lineage, &target, TargetMode::Save, &mut survey) {
        Ok(()) | Err(Refusal::TooLarge) => {}
        Err(other) => panic!("a hostile cache folder was refused as {other:?}"),
    }
    let Ok(surveyed) = survey.finish() else { return };
    inspect(&surveyed, &note, content);
}

fn inspect(surveyed: &Surveyed, note: &Path, content: &[u8]) {
    let inventory = surveyed.inventory();
    let mut own = None;
    for artifact in inventory.artifacts() {
        assert!(OURS.contains(&artifact.store), "a store this survey doesn't own: {:?}", artifact.store);
        assert_ne!(artifact.owner, Owner::Unrelated, "nothing in the caches can be proved another document's");
        let (_, key) = surveyed.key(artifact.id).expect("every artifact has a key");
        let named = name_key(Store::CorpusAliases, note);
        if artifact.store == Store::CorpusAliases && key == named {
            own = Some(*artifact);
        }
    }
    // The note named for the target's path is the target's whenever it could be read (a private plain file of one link: a hard link to it is shared).
    if let Some(own) = own {
        assert!(matches!(own.owner, Owner::Target | Owner::Shared), "the target's own note was {:?}", own.owner);
        assert_eq!(
            own.format == Format::Current,
            is_the_apps_format(content),
            "a note's format and what it holds disagree"
        );
    }
}

fn name_key(store: Store, note: &Path) -> recto_core::removal::ArtifactKey {
    let mut name = b"by-file/".to_vec();
    name.extend_from_slice(note.file_name().expect("a name").as_encoded_bytes());
    recto_core::removal::ArtifactKey::of(store, &name)
}
