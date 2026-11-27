//! The names `names.rs` restates, held against the code that writes them. A writer that renames one fails here, where the survey would otherwise read its
//! files as strangers (`Unknown`, a refusal nobody can clear) or, worse, skip them.

use super::tests::World;
use super::*;
use recto_index::corpus::{self, CorpusPaths};
use std::fs;

fn names_in(folder: &Path) -> Vec<String> {
    let mut found: Vec<String> = fs::read_dir(folder).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    found.sort();
    found
}

/// Failure: the app renames its agent's folder, or the one it sets aside at a session's end, and the survey stops seeing them (an agent's pages stay on disk
/// after a removal). Held against `claim_agent_folder` and `retire_agent_folder`, which make the folder and move it aside.
#[test]
fn the_agents_folder_names_are_what_the_writers_make() {
    let w = World::new();
    let key = w.index(&w.mine);
    let paths = CorpusPaths::new(&w.corpus, key);
    assert_eq!(paths.agent.file_name().unwrap(), "agent");
    assert_eq!(paths.agent.parent().unwrap().file_name().unwrap(), corpus::FORMAT);
    let format_dir = paths.agent.parent().unwrap().to_path_buf();

    let claim = corpus::claim_agent_folder(&w.corpus, key).expect("claimed");
    let live: Vec<String> = names_in(&format_dir).into_iter().filter(|name| name.starts_with("agent")).collect();
    assert_eq!(live, ["agent"]);
    assert!(names::is_agent_folder(OsStr::new(&live[0])));

    corpus::retire_agent_folder(&w.corpus, key, claim);
    let aside: Vec<String> = names_in(&format_dir).into_iter().filter(|name| name.starts_with("agent")).collect();
    assert_eq!(aside.len(), 1);
    assert!(
        aside[0].starts_with("agent.gone.") && names::is_agent_folder(OsStr::new(&aside[0])),
        "{aside:?}"
    );
    let seen = w.seen(TargetMode::Save);
    assert_eq!(
        seen.owner(Store::CorpusAgentPages, &format!("{}/{}/{}", key.hex(), corpus::FORMAT, aside[0])),
        Owner::Target
    );
}

/// Failure: the app renames one of its two markers in the corpus root, and the survey calls it a stranger. Held against `remove_plain_corpora` (which writes
/// `sealed`) and `note_agent_folder` (which writes `agent-folders`), and against the temporary `with_extension` makes of each.
#[test]
fn the_markers_are_what_the_writers_make() {
    let w = World::new();
    w.index(&w.mine);
    corpus::remove_plain_corpora(&w.corpus);
    corpus::note_agent_folder(&w.corpus);
    let markers: Vec<String> = names_in(&w.corpus).into_iter().filter(|name| name.len() != 64 && name != "by-file").collect();
    assert_eq!(markers, ["agent-folders", "sealed"], "the root's names besides the corpora");
    assert_eq!(markers, {
        let mut ours = names::MARKERS.to_vec();
        ours.sort();
        ours
    });
    for marker in &markers {
        assert_eq!(names::root_name(OsStr::new(marker)), names::RootName::Marker);
        let temp = w.corpus.join(marker).with_extension(format!("tmp{}", std::process::id()));
        assert_eq!(names::root_name(temp.file_name().unwrap()), names::RootName::Marker, "the temp of {marker}");
    }
    assert_eq!(
        w.seen(TargetMode::Save).count(Store::CorpusText),
        1,
        "the markers aren't artifacts: only the corpus is"
    );
}

/// Failure: the app changes how a file is named in `by-file`, and the survey misses the notes that name the target, so the old versions' text stays. A key
/// planted under the survey's name for the path, and for the inode, is found by `keys_of_file`, which reads the notes by the app's own names. Also the
/// temporary `with_extension` makes of a note's name.
#[test]
fn the_notes_names_are_what_keys_of_file_reads() {
    let w = World::new();
    let (by_path, by_inode) = (hex_key(0x31), hex_key(0x32));
    let (device, inode) = w.target().device_inode.unwrap();
    w.note(&names::path_alias(w.mine.as_os_str().as_encoded_bytes()), by_path.as_bytes());
    w.note(&names::inode_alias(device, inode), by_inode.as_bytes());
    let keys: Vec<String> = corpus::keys_of_file(&w.corpus, &w.mine).into_iter().map(|key| key.hex()).collect();
    assert!(keys.contains(&by_path), "the path's name: {keys:?}");
    assert!(keys.contains(&by_inode), "the inode's name: {keys:?}");
    let note = w.corpus.join(names::VERSIONS).join(names::path_alias(w.mine.as_os_str().as_encoded_bytes()));
    let temp = note.with_extension(format!("tmp{}", std::process::id()));
    assert_eq!(
        names::alias_stem(temp.file_name().unwrap()),
        Some(names::path_alias(w.mine.as_os_str().as_encoded_bytes()).as_str())
    );
}

/// Failure: the app changes a kept contents file's name or its tag, and the survey stops placing it, so a removed document's headings stay. Held against
/// `generated::write`, and against the temporary `with_extension` makes of its name.
#[test]
fn the_contents_name_is_what_generated_write_makes() {
    let w = World::new();
    let key = CorpusKey::of_file(&w.mine).unwrap();
    w.keep_contents(&w.mine, key);
    let kept = names_in(&w.contents);
    assert_eq!(kept.len(), 1);
    assert_eq!(
        kept[0],
        format!("{}.{}.json", names::contents_tag(w.mine.as_os_str().as_encoded_bytes()), key.hex())
    );
    let (tag, found) = names::contents_name(OsStr::new(&kept[0])).expect("a name of ours");
    assert_eq!((tag, found), (names::contents_tag(w.mine.as_os_str().as_encoded_bytes()).as_str(), key));
    let temp = w.contents.join(&kept[0]).with_extension(format!("tmp{}", std::process::id()));
    assert!(names::contents_name(temp.file_name().unwrap()).is_some(), "the temp of a write: {temp:?}");
    assert_eq!(w.seen(TargetMode::Save).owner(Store::GeneratedContents, &kept[0]), Owner::Target);
}

/// Failure: the app moves the plain-text folder of its first build, or the corpus root's name changes, and a removal leaves the old words behind unseen. Held
/// against `old_folder_is_there` and against the name `StatePaths` gives the corpus root.
#[test]
fn the_old_folder_is_where_the_app_looks_for_it() {
    let w = World::new();
    fs::create_dir_all(&w.corpus).unwrap();
    assert!(!corpus::old_folder_is_there(&w.corpus));
    fs::create_dir(w.dir.join("corpus")).unwrap();
    assert!(corpus::old_folder_is_there(&w.corpus), "a folder `corpus` beside the corpus root");
    assert_eq!(w.seen(TargetMode::Save).owner(Store::CorpusText, "old/corpus"), Owner::Unknown);
    let paths = crate::state::StatePaths::overridden(&w.dir.join("state")).unwrap();
    assert_eq!(
        paths.corpus.file_name().unwrap(),
        names::CORPUS_ROOT_NAME,
        "the name the app gives the corpus root"
    );
    assert_eq!(paths.corpus.parent().unwrap(), paths.generated.parent().unwrap());
    assert_eq!(paths.generated.file_name().unwrap(), "contents.noindex");
}

fn hex_key(byte: u8) -> String {
    format!("{byte:02x}").repeat(32)
}
