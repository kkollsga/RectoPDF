//! The kept copy's temporary, in a temp state root: each test names the failure it catches.

use super::*;
use crate::state::StatePaths;
use recto_testpdf::TempDir;
use std::sync::Arc;

fn five_pages() -> Vec<u8> {
    let pages: Vec<recto_testpdf::Page> = (0..5)
        .map(|at| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("PAGEMARK{at}")])))
        .collect();
    recto_testpdf::pages(&pages)
}

/// Failure: a kill between the kept copy's write and its rename leaves up to 64 MiB of the pages a save took out, under a name nothing sweeps: outside the
/// 30-day prune for ever, and a removal refuses on it (an unplaced name is `Unknown`). The copy is written under the store's own temp name, which the launch
/// sweep (`remove_temp_files`) removes: a leftover made as a kill makes it is gone after the next load.
#[test]
fn a_kill_between_the_kept_copy_and_its_rename_leaves_a_temp_the_next_load_removes() {
    let dir = TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-kept-");
    let paths = StatePaths::overridden(&dir).unwrap();
    paths.prepare().unwrap();
    let store = Arc::new(RecordStore::open(&paths, Flavor::App));
    let file = dir.join("doc.pdf");
    std::fs::write(&file, five_pages()).unwrap();
    let target = FileStamp {
        size: 9,
        mtime_ns: 1,
        device: 1,
        inode: 2,
    };
    let folder = dir.join("session").join(JOURNALS);
    let aside = dir.join("aside");
    std::fs::create_dir(&aside).unwrap();
    // The copy is done and not yet renamed: what a kill there leaves is what is in the folder now, so it is set aside and put back after.
    hold::park::set(
        &store.root,
        "kept",
        Arc::new({
            let (folder, aside) = (folder.clone(), aside.clone());
            move || {
                for entry in std::fs::read_dir(&folder).unwrap().flatten() {
                    if entry.file_name().to_string_lossy().contains("tmp") {
                        std::fs::copy(entry.path(), aside.join(entry.file_name())).unwrap();
                    }
                }
            }
        }),
    );
    assert!(store.keep_pages_of(&file, target, &[1, 3]).is_some());
    hold::park::clear(&store.root);
    let leftovers: Vec<_> = std::fs::read_dir(&aside).unwrap().flatten().collect();
    assert_eq!(leftovers.len(), 1, "one temporary was there mid-copy");
    let name = leftovers[0].file_name();
    std::fs::copy(leftovers[0].path(), folder.join(&name)).unwrap();
    drop(store);
    let next = RecordStore::open(&paths, Flavor::App);
    let _ = next.load();
    assert!(!folder.join(&name).exists(), "the next launch's sweep removed {name:?}");
}
