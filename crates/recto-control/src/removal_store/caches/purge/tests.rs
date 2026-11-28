//! The purge of the four cache stores over real folders, with a real clear-all survey and a real plan; each test names the failure it catches. Quiet is stood
//! for by running no writer, and not-quiet by holding the real locks (a live agent claim, a held `build.lock`).

use super::*;
use crate::removal_store::caches::CacheCaps;
use crate::removal_store::caches::tests::{OURS, World, dir, hex_key, plain};
use crate::removal_store::coverage_tests::plan_of;
use crate::removal_store::purge::Pins;
use recto_core::removal::{ArtifactId, Effect, Surveyed};
use recto_index::corpus::{self, CorpusKey};
use std::fs;
use std::os::unix::fs::{MetadataExt as _, symlink};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

type Step = (Store, Vec<(ArtifactKey, Pin)>);

fn volume(w: &World) -> VolumeNow {
    VolumeNow {
        root: 0,
        dev: fs::metadata(w.dir.path()).unwrap().dev(),
    }
}

fn roots(w: &World) -> CacheRoots<'_> {
    CacheRoots {
        corpus: &w.corpus,
        contents: &w.contents,
    }
}

fn wanted(s: &Surveyed, pins: &Pins, ids: &[ArtifactId]) -> Vec<(ArtifactKey, Pin)> {
    ids.iter()
        .map(|id| {
            let (_, key) = s.key(*id).unwrap();
            let pinned = pins.get(&key).expect("a cleared artifact is pinned");
            let kind = if pinned.kind == Kind::Folder { PinKind::Folder } else { PinKind::File };
            (
                key,
                Pin {
                    root: 0,
                    inode: pinned.ino,
                    kind,
                },
            )
        })
        .collect()
}

/// A real clear-all survey of the world, the plan of a removal over it, and every step of the four cache stores in the plan's order, as the barrier would
/// record them (this volume is root 0).
fn plan_steps(w: &World) -> Vec<Step> {
    let (s, pins) = w.clear_survey(&CacheCaps::default()).unwrap();
    let p = plan_of(&s).expect("a plan");
    p.steps()
        .iter()
        .filter_map(|step| match step.effect() {
            Effect::Purge(store) | Effect::Rewrite(store) if OURS.contains(&store) => Some((store, wanted(&s, &pins, p.artifacts(step)))),
            _ => None,
        })
        .collect()
}

fn step_of(steps: &[Step], store: Store) -> Vec<(ArtifactKey, Pin)> {
    steps.iter().filter(|(s, _)| *s == store).flat_map(|(_, wanted)| wanted.clone()).collect()
}

fn purge(w: &World, store: Store, wanted: &[(ArtifactKey, Pin)], seams: &CacheSeams) -> Result<Purged, PurgeStop> {
    purge_caches_with(store, wanted, &roots(w), volume(w), &CachesQuiet::assumed_for_tests(), seams)
}

/// Every step in the plan's order through the purge.
fn purge_all(w: &World, steps: &[Step], seams: &CacheSeams) -> Vec<Result<Purged, PurgeStop>> {
    steps.iter().map(|(store, wanted)| purge(w, *store, wanted, seams)).collect()
}

fn syncs(seams: &CacheSeams) -> u32 {
    seams.files.syncs.load(Ordering::SeqCst)
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// Three documents' corpora, built by the real indexer; their keys in the order the purge takes them (sorted by name).
fn three_corpora(w: &World) -> Vec<CorpusKey> {
    let third = w.dir.join("docs").join("third.pdf");
    fs::write(&third, b"%PDF-1.4 third, the longest of the three").unwrap();
    let mut keys: Vec<CorpusKey> = [&w.mine, &w.theirs, &third].into_iter().map(|path| w.index(path)).collect();
    keys.sort_by_key(|key| key.hex());
    keys
}

fn hold(path: &Path) -> fs::File {
    let file = fs::OpenOptions::new().append(true).open(path).unwrap();
    file.try_lock().expect("the test takes the lock");
    file
}

fn build_lock(w: &World, key: CorpusKey) -> PathBuf {
    w.corpus.join(key.hex()).join("v4").join("app").join("build.lock")
}

fn seams_with(change: impl FnOnce(&mut CacheSeams)) -> CacheSeams {
    let mut seams = CacheSeams::default();
    change(&mut seams);
    seams
}

/// Failure: a step of the plan leaves cache behind (a note's or contents file's temp left out of the key function, a set-aside agent folder, a corpus), or takes
/// what is not the cache's: the person's file a link leads to, a second name of a file, a stray in a sibling folder. A real survey, plan and every cache step in
/// the plan's order, then `caches_are_clear` is true, and what is outside the cache is as it was.
#[test]
fn a_plans_cache_steps_leave_no_cache() {
    let w = World::new();
    let (mine, theirs) = (w.index(&w.mine), w.index(&w.theirs));
    w.keep_contents(&w.mine, mine);
    w.keep_contents(&w.theirs, theirs);
    for key in [mine, theirs] {
        let claim = corpus::claim_agent_folder(&w.corpus, key).unwrap();
        corpus::retire_agent_folder(&w.corpus, key, claim);
    }
    plain(&w.corpus.join("by-file").join(format!("{}.tmp5", hex_key(0x41))));
    plain(&w.contents.join(format!("0123456789abcdef.{}.tmp5", hex_key(0x42))));
    corpus::note_agent_folder(&w.corpus);
    let (persons, sibling) = (w.dir.join("docs").join("persons-link"), w.dir.join("sibling").join("stray"));
    symlink(&w.mine, &persons).unwrap();
    fs::hard_link(&w.mine, w.dir.join("docs").join("mine-second-name")).unwrap();
    plain(&sibling);
    let steps = plan_steps(&w);
    assert_eq!(steps.len(), 4, "one step a store: {steps:?}");
    for result in purge_all(&w, &steps, &CacheSeams::default()) {
        assert!(result.unwrap().durable);
    }
    assert_eq!(super::super::caches_are_clear(&w.corpus, &w.contents), Ok(true));
    assert!(
        exists(&persons) && fs::read(&w.mine).unwrap().starts_with(b"%PDF"),
        "a link's target is untouched"
    );
    assert!(w.dir.join("docs").join("mine-second-name").exists() && exists(&sibling));
    assert!(w.corpus.join("agent-folders").exists(), "a marker holds no document data and stays");
}

/// Failure: the agent-pages step errors on what it normally meets (its corpus already taken by the `CorpusText` step), or never confirms none is left, or when
/// run alone fails to find an agent folder set aside. After its corpus: `Ok`, every one already gone, the root synced. Alone: it takes the agent folders by their
/// own trees (the live and the set-aside), and leaves the text.
#[test]
fn the_agent_pages_step_after_its_corpus_finds_them_gone() {
    let w = World::new();
    let (mine, theirs) = (w.index(&w.mine), w.index(&w.theirs));
    for key in [mine, theirs] {
        let claim = corpus::claim_agent_folder(&w.corpus, key).unwrap();
        corpus::retire_agent_folder(&w.corpus, key, claim);
    }
    drop(corpus::claim_agent_folder(&w.corpus, mine).unwrap());
    let steps = plan_steps(&w);
    let agents = step_of(&steps, Store::CorpusAgentPages);
    assert_eq!(agents.len(), 3);
    // Alone first, on this world: only the agent folders go.
    let alone = seams_with(|_| {});
    let purged = purge(&w, Store::CorpusAgentPages, &agents, &alone).unwrap();
    assert_eq!((purged.removed, purged.already_gone), (3, 0));
    assert!(w.corpus.join(mine.hex()).join("v4").join("app").join("text.pack").exists(), "the text stays");
    assert!(!w.corpus.join(mine.hex()).join("v4").join("agent").exists());
    // Then its corpus step, then the agents again: all gone, and said so.
    purge(&w, Store::CorpusText, &step_of(&steps, Store::CorpusText), &alone).unwrap();
    let after = seams_with(|_| {});
    let again = purge(&w, Store::CorpusAgentPages, &agents, &after).unwrap();
    assert_eq!((again.removed, again.already_gone, again.durable), (0, 3, true));
    assert!(syncs(&after) >= 1, "the root is synced when something was found gone");
    // And with the corpus root itself missing.
    fs::remove_dir_all(&w.corpus).unwrap();
    assert_eq!(purge(&w, Store::CorpusAgentPages, &agents, &CacheSeams::default()).unwrap().already_gone, 3);
}

/// Failure: a corpus that a build or an agent session holds is unlinked, or a stop leaves some of the step gone before it is seen. A live claim on the second of
/// three corpora, and then a held `build.lock`, are `Busy` and all three are there. A lock taken between pass 1 and pass 2 stops at that folder: the earlier ones
/// are gone, the root is synced.
#[test]
fn a_held_build_lock_or_agent_session_stops_before_any_unlink() {
    let w = World::new();
    let keys = three_corpora(&w);
    let steps = plan_steps(&w);
    let corpora = step_of(&steps, Store::CorpusText);
    let all_there = |w: &World| keys.iter().all(|key| w.corpus.join(key.hex()).exists());
    let claim = corpus::claim_agent_folder(&w.corpus, keys[1]).expect("a live session");
    assert_eq!(purge(&w, Store::CorpusText, &corpora, &CacheSeams::default()), Err(PurgeStop::Busy));
    assert!(all_there(&w), "an agent session's lock stops the step before any unlink");
    drop(claim);
    let build = hold(&build_lock(&w, keys[2]));
    assert_eq!(purge(&w, Store::CorpusText, &corpora, &CacheSeams::default()), Err(PurgeStop::Busy));
    assert!(all_there(&w), "a build's lock stops it too, though it is the last corpus");
    drop(build);
    // Taken between the two passes: the corpora before it are gone, it is not.
    let held = Arc::new(std::sync::Mutex::new(None));
    let last = build_lock(&w, keys[2]);
    let seams = seams_with(|s| {
        let held = held.clone();
        s.between_passes = Some(Arc::new(move |_| *held.lock().unwrap() = Some(hold(&last))));
    });
    assert_eq!(purge(&w, Store::CorpusText, &corpora, &seams), Err(PurgeStop::Busy));
    assert!(
        !w.corpus.join(keys[0].hex()).exists() && !w.corpus.join(keys[1].hex()).exists(),
        "the earlier ones went"
    );
    assert!(w.corpus.join(keys[2].hex()).exists(), "the held one stays");
    assert!(syncs(&seams) >= 1, "the root was synced on the way out");
}

/// Failure: a step acts on a name that is now another thing: a key folder replaced by another of the same name, a link planted inside a corpus after the survey,
/// or a note rewritten to a new inode, is unlinked because the name matched. Each is `NotThePinned` and nothing in the step is unlinked.
#[test]
fn a_folder_or_file_replaced_after_the_survey_stops_before_any_unlink() {
    let w = World::new();
    let keys = three_corpora(&w);
    let steps = plan_steps(&w);
    let corpora = step_of(&steps, Store::CorpusText);
    let moved = w.dir.join("moved");
    fs::rename(w.corpus.join(keys[1].hex()), &moved).unwrap();
    dir(&w.corpus.join(keys[1].hex()));
    assert_eq!(purge(&w, Store::CorpusText, &corpora, &CacheSeams::default()), Err(PurgeStop::NotThePinned));
    assert!(
        w.corpus.join(keys[0].hex()).exists() && w.corpus.join(keys[2].hex()).exists(),
        "nothing was unlinked"
    );
    fs::remove_dir(w.corpus.join(keys[1].hex())).unwrap();
    fs::rename(&moved, w.corpus.join(keys[1].hex())).unwrap();
    // A link planted after the survey, in the last corpus: found when its tree is built, so the earlier ones go and this one stays whole.
    let persons = w.dir.join("persons-file");
    fs::write(&persons, b"mine").unwrap();
    let planted = w.corpus.join(keys[2].hex()).join("v4").join("app").join("planted");
    symlink(&persons, &planted).unwrap();
    assert_eq!(purge(&w, Store::CorpusText, &corpora, &CacheSeams::default()), Err(PurgeStop::NotThePinned));
    assert!(
        w.corpus.join(keys[2].hex()).join("v4").join("app").join("ready.sealed").exists(),
        "the corpus with a link in it is whole"
    );
    assert_eq!(fs::read(&persons).unwrap(), b"mine");
    // A note rewritten to a new inode.
    let w = World::new();
    three_corpora(&w);
    let steps = plan_steps(&w);
    let notes = step_of(&steps, Store::CorpusAliases);
    let note = fs::read_dir(w.corpus.join("by-file")).unwrap().next().unwrap().unwrap().path();
    fs::remove_file(&note).unwrap();
    fs::write(&note, b"rewritten").unwrap();
    assert_eq!(purge(&w, Store::CorpusAliases, &notes, &CacheSeams::default()), Err(PurgeStop::NotThePinned));
    assert_eq!(fs::read_dir(w.corpus.join("by-file")).unwrap().count(), notes.len(), "no note was unlinked");
}

/// Failure: a name swapped for a link between the listing and the unlink is unlinked or followed. A note swapped at its own unlink stops (`NotAFile`): the
/// person's file and the link stay, the earlier notes are gone and the folder is synced. In a tree, `pages.bin` (which sorts before `ready.sealed`) swapped at the walk's gate stops
/// (`NotThePinned`): the link and its target stay, and `ready.sealed`, which goes first, is already gone, with the root synced.
#[test]
fn a_swap_between_list_and_unlink_stops_there() {
    let w = World::new();
    let keys = three_corpora(&w);
    let persons = w.dir.join("persons-file");
    fs::write(&persons, b"mine").unwrap();
    let steps = plan_steps(&w);
    let notes = step_of(&steps, Store::CorpusAliases);
    let mut names: Vec<_> = fs::read_dir(w.corpus.join("by-file")).unwrap().map(|e| e.unwrap().file_name()).collect();
    names.sort();
    let last = w.corpus.join("by-file").join(names.last().unwrap());
    let seams = seams_with(|s| {
        let (last, persons) = (last.clone(), persons.clone());
        s.files.before_unlink = Some(Arc::new(move |at: &Path| {
            if at == last {
                fs::remove_file(at).unwrap();
                symlink(&persons, at).unwrap();
            }
        }));
    });
    assert_eq!(purge(&w, Store::CorpusAliases, &notes, &seams), Err(PurgeStop::NotAFile));
    assert!(exists(&last) && fs::read(&persons).unwrap() == b"mine", "the link and its target stay");
    assert_eq!(fs::read_dir(w.corpus.join("by-file")).unwrap().count(), 1, "the earlier notes are gone");
    assert_eq!(syncs(&seams), 1, "and the folder that lost them was synced");
    // The tree.
    let corpora = step_of(&steps, Store::CorpusText);
    let app = w.corpus.join(keys[0].hex()).join("v4").join("app");
    let seams = seams_with(|s| {
        let (pack, persons) = (app.join("pages.bin"), persons.clone());
        s.gate = Some(Arc::new(move |name: &OsStr| {
            if name == "pages.bin" && pack.exists() && !pack.is_symlink() {
                fs::remove_file(&pack).unwrap();
                symlink(&persons, &pack).unwrap();
            }
            true
        }));
    });
    assert_eq!(purge(&w, Store::CorpusText, &corpora, &seams), Err(PurgeStop::NotThePinned));
    assert!(
        app.join("pages.bin").is_symlink() && fs::read(&persons).unwrap() == b"mine",
        "the link and its target stay"
    );
    assert!(
        !app.join("ready.sealed").exists(),
        "what makes a corpus usable went first, so no search reads what is left"
    );
    assert!(syncs(&seams) >= 1, "the root is synced on the way out");
}

/// Failure: a file that got a second name after the survey is unlinked (the person's other name keeps the file, but the plan promised to clear a thing nobody
/// else holds). The step stops `Linked`, and both names stay.
#[test]
fn a_hard_link_made_after_the_survey_stops_the_step() {
    let w = World::new();
    three_corpora(&w);
    let steps = plan_steps(&w);
    let kept = step_of(&steps, Store::CorpusAliases);
    let note = fs::read_dir(w.corpus.join("by-file")).unwrap().next().unwrap().unwrap().path();
    let other = w.dir.join("second-name");
    fs::hard_link(&note, &other).unwrap();
    assert_eq!(purge(&w, Store::CorpusAliases, &kept, &CacheSeams::default()), Err(PurgeStop::Linked));
    assert!(note.exists() && other.exists());
}

/// Failure: a stop part way leaves what lost a name unsynced, a rerun after a kill fails for what is gone, or a repeat after success isn't `Ok`. A stop in a
/// tree (the gate) syncs the root; a rerun takes the rest and counts the gone ones; a repeat is `Ok` with everything already gone and the root synced again; and
/// a sync that fails is not durable.
#[test]
fn every_way_out_syncs_what_lost_a_name_and_a_repeated_step_succeeds() {
    let w = World::new();
    let keys = three_corpora(&w);
    let steps = plan_steps(&w);
    let corpora = step_of(&steps, Store::CorpusText);
    // The gate stops the walk at the second corpus's `text.pack`: the first corpus is gone, the second has lost what made it usable, the third is whole.
    let packs = Arc::new(AtomicU32::new(0));
    let stopped = seams_with(|s| {
        let packs = packs.clone();
        s.gate = Some(Arc::new(move |name: &OsStr| name != "text.pack" || packs.fetch_add(1, Ordering::SeqCst) == 0));
    });
    assert_eq!(purge(&w, Store::CorpusText, &corpora, &stopped), Err(PurgeStop::Listing));
    assert!(!w.corpus.join(keys[0].hex()).exists() && w.corpus.join(keys[1].hex()).exists() && w.corpus.join(keys[2].hex()).exists());
    assert!(
        !w.corpus.join(keys[1].hex()).join("v4").join("app").join("ready.sealed").exists(),
        "no search reads the stopped one"
    );
    assert!(syncs(&stopped) >= 1, "the root lost a name and was synced on the way out");
    let rerun = seams_with(|_| {});
    let done = purge(&w, Store::CorpusText, &corpora, &rerun).unwrap();
    assert_eq!((done.removed, done.already_gone, done.durable), (2, 1, true));
    let repeat = seams_with(|_| {});
    let again = purge(&w, Store::CorpusText, &corpora, &repeat).unwrap();
    assert_eq!((again.removed, again.already_gone, again.durable), (0, 3, true));
    assert!(
        syncs(&repeat) >= 1,
        "everything was gone, and a killed run may not have synced: the root is synced"
    );
    let failing = seams_with(|s| s.files.fail_sync = true);
    assert!(!purge(&w, Store::CorpusText, &corpora, &failing).unwrap().durable);
}

/// Moves the folder at `dir` aside and puts a new one at its name: the name no longer leads to the folder that was pinned.
fn swap_for_a_new_folder(dir: &Path) {
    fs::rename(dir, dir.with_file_name("aside-of-the-pinned")).unwrap();
    fs::create_dir(dir).unwrap();
}

/// Failure: a corpora step that stops in pass 1 (a folder that is no longer the pinned one) leaves the root unsynced though a wanted name was already gone,
/// which a run killed before its sync may have taken. One wanted corpus is removed and another replaced by a new folder at its name: the step stops
/// `NotThePinned`, the third is untouched, and the root is synced on the way out.
#[test]
fn a_corpora_stop_in_pass_one_still_syncs_the_root_when_a_wanted_name_was_already_gone() {
    let w = World::new();
    let keys = three_corpora(&w);
    let corpora = step_of(&plan_steps(&w), Store::CorpusText);
    fs::remove_dir_all(w.corpus.join(keys[0].hex())).unwrap();
    swap_for_a_new_folder(&w.corpus.join(keys[1].hex()));
    let seams = CacheSeams::default();
    assert_eq!(purge(&w, Store::CorpusText, &corpora, &seams), Err(PurgeStop::NotThePinned));
    assert!(w.corpus.join(keys[2].hex()).exists(), "nothing was unlinked");
    assert!(syncs(&seams) >= 1, "the root is synced when a name was already gone");
}

/// Failure: an agent-pages step that stops in pass 1 leaves the root unsynced though a wanted agent folder was already gone. One is removed and another replaced
/// by a new folder at its name: the step stops `NotThePinned`, the replaced one is untouched, and the root is synced on the way out.
#[test]
fn an_agent_pages_stop_in_pass_one_still_syncs_the_root_when_a_wanted_name_was_already_gone() {
    let w = World::new();
    let keys = three_corpora(&w);
    for key in &keys {
        let claim = corpus::claim_agent_folder(&w.corpus, *key).unwrap();
        corpus::retire_agent_folder(&w.corpus, *key, claim);
    }
    let agents = step_of(&plan_steps(&w), Store::CorpusAgentPages);
    assert_eq!(agents.len(), 3);
    let agent_dir = |key: CorpusKey| {
        fs::read_dir(w.corpus.join(key.hex()).join("v4"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.file_name().is_some_and(names::is_agent_folder))
            .unwrap()
    };
    fs::remove_dir_all(agent_dir(keys[0])).unwrap();
    let swapped = agent_dir(keys[1]);
    swap_for_a_new_folder(&swapped);
    let seams = CacheSeams::default();
    assert_eq!(purge(&w, Store::CorpusAgentPages, &agents, &seams), Err(PurgeStop::NotThePinned));
    assert!(swapped.exists());
    assert!(syncs(&seams) >= 1, "the root is synced when a name was already gone");
}

/// Failure: a step runs with a pin that isn't its store's kind, a pin on another volume, a key twice, a batch over the limit, or for a store that isn't a cache,
/// and unlinks something. Each is refused whole, and every file stays.
#[test]
fn a_bad_pin_another_volume_a_key_twice_an_oversize_batch_or_another_store_does_nothing() {
    let w = World::new();
    three_corpora(&w);
    let steps = plan_steps(&w);
    let (corpora, notes) = (step_of(&steps, Store::CorpusText), step_of(&steps, Store::CorpusAliases));
    let before = fs::read_dir(&w.corpus).unwrap().count();
    let as_file: Vec<_> = corpora.iter().map(|(key, pin)| (*key, Pin { kind: PinKind::File, ..*pin })).collect();
    let other_volume: Vec<_> = corpora.iter().map(|(key, pin)| (*key, Pin { root: 1, ..*pin })).collect();
    let twice = [corpora[0], corpora[0]];
    let oversize: Vec<_> = (0..129u32)
        .map(|n| (ArtifactKey::of(Store::CorpusText, &n.to_le_bytes()), corpora[0].1))
        .collect();
    let quiet = CacheSeams::default();
    assert_eq!(purge(&w, Store::CorpusText, &as_file, &quiet), Err(PurgeStop::BadPin));
    assert_eq!(purge(&w, Store::CorpusText, &other_volume, &quiet), Err(PurgeStop::OtherVolume));
    assert_eq!(purge(&w, Store::CorpusText, &twice, &quiet), Err(PurgeStop::BadPin));
    assert_eq!(purge(&w, Store::CorpusText, &oversize, &quiet), Err(PurgeStop::TooLarge));
    assert_eq!(
        purge(&w, Store::CorpusAliases, &corpora, &quiet),
        Err(PurgeStop::BadPin),
        "a folder's pin for a file store"
    );
    assert_eq!(purge(&w, Store::Journal, &notes, &quiet), Err(PurgeStop::NotCache));
    assert_eq!(fs::read_dir(&w.corpus).unwrap().count(), before);
    assert!(
        corpora
            .iter()
            .all(|(key, _)| plan_steps(&w).iter().any(|(_, wanted)| wanted.iter().any(|(k, _)| k == key))),
        "all still surveyed"
    );
}

/// Failure: the older build's `corpus` (a level deeper than a pin holds) is left behind, refused for its depth, or taken whole in one walk. It goes child by
/// child: a file child by a look and an unlink, a key folder by its own tree, and `corpus` itself last as a folder, with the folder beside it synced.
#[test]
fn the_older_builds_corpus_goes_child_by_child() {
    let w = World::new();
    let key = w.index(&w.mine);
    let old = w.dir.join("corpus");
    let pages = old.join(hex_key(0x51)).join("v1").join("agent").join("pages");
    dir(&pages);
    fs::write(pages.join("0001.md"), b"page").unwrap();
    plain(&old.join("loose.txt"));
    let steps = plan_steps(&w);
    let seams = CacheSeams::default();
    let result = purge(&w, Store::CorpusText, &step_of(&steps, Store::CorpusText), &seams).unwrap();
    assert_eq!(result.removed, 2, "the corpus and the old folder");
    assert!(!old.exists() && !w.corpus.join(key.hex()).exists());
    assert!(syncs(&seams) >= 2, "the corpus root, and the folder beside it");
}

/// Failure: the purge holds more descriptors than the bound (a listing, a lock or a folder kept open past its use), or keeps every corpus's tree: at every unlink
/// of a walk over 16 corpora the process holds at most 8 descriptors more than before it started.
#[test]
fn the_purge_holds_at_most_eight_descriptors() {
    fn open_descriptors() -> usize {
        fs::read_dir("/dev/fd").unwrap().count()
    }
    let w = World::new();
    for byte in 0..16u8 {
        w.key_folder(&hex_key(0x60 + byte));
        let key = hex_key(0x60 + byte);
        let agent = w.corpus.join(&key).join("v4").join("agent").join("pages");
        dir(&agent);
        fs::write(agent.join("0001.md"), b"p").unwrap();
        fs::write(w.corpus.join(&key).join("v4").join("app").join("ready.sealed"), b"r").unwrap();
        fs::write(w.corpus.join(&key).join("v4").join("app").join("build.lock"), b"").unwrap();
    }
    let peak = Arc::new(AtomicU32::new(0));
    let steps = plan_steps(&w);
    let baseline = open_descriptors();
    // The deepest holds are while a tree is built: the survey's rule asks who owns every entry it meets, so that question is where they are counted too.
    let in_tree = {
        let peak = peak.clone();
        Arc::new(move |_: &OsStr| {
            peak.fetch_max(open_descriptors() as u32, Ordering::SeqCst);
            None
        })
    };
    let seams = seams_with(|s| {
        s.uid_of = Some(in_tree);
        let at_gate = peak.clone();
        s.gate = Some(Arc::new(move |_| {
            at_gate.fetch_max(open_descriptors() as u32, Ordering::SeqCst);
            true
        }));
        let at_unlink = peak.clone();
        s.unlinked = Some(Arc::new(move || {
            at_unlink.fetch_max(open_descriptors() as u32, Ordering::SeqCst);
        }));
    });
    for (store, wanted) in &steps {
        if matches!(store, Store::CorpusText | Store::CorpusAgentPages) {
            purge(&w, *store, wanted, &seams).unwrap();
        }
    }
    let most = peak.load(Ordering::SeqCst) as usize;
    assert!(most > baseline, "the gate ran with folders open");
    assert!(most - baseline <= 8, "{} more than before", most - baseline);
}

/// Failure: the last walk is missing or reads the first listing again, so a folder that is there again once the step is done (a build or a session made it
/// again in the gap) is reported as taken. The corpus's folder and the agent folder are each made again at the unlink that takes them, and the step is
/// `StillThere`.
#[test]
fn the_last_walk_finds_what_came_back() {
    let w = World::new();
    let key = w.index(&w.mine);
    drop(corpus::claim_agent_folder(&w.corpus, key).unwrap());
    let steps = plan_steps(&w);
    let (corpus_dir, agent_dir) = (w.corpus.join(key.hex()), w.corpus.join(key.hex()).join("v4").join("agent"));
    let again = |at: PathBuf| {
        seams_with(|s| {
            s.unlinked = Some(Arc::new(move || {
                if !at.exists() {
                    fs::create_dir(&at).unwrap();
                }
            }))
        })
    };
    assert_eq!(
        purge(&w, Store::CorpusAgentPages, &step_of(&steps, Store::CorpusAgentPages), &again(agent_dir)),
        Err(PurgeStop::StillThere)
    );
    assert_eq!(
        purge(&w, Store::CorpusText, &step_of(&steps, Store::CorpusText), &again(corpus_dir)),
        Err(PurgeStop::StillThere)
    );
}
