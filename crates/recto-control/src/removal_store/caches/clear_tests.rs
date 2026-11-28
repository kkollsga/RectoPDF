//! The clear-all survey over real folders (`survey_caches_to_clear`): what a confirmed removal clears of every document's Find text, what refuses it, and the
//! pin of each thing it will take away. Each test names the failure it catches.

use super::tests::{OURS, Seen, World, dir, hex_key, link, pipe, plain, small, two_documents, two_links};
use super::*;
use crate::removal_store::coverage_tests::plan_of;
use recto_core::removal::Surveyed;
use recto_index::corpus;
use std::fs;
use std::os::unix::fs::symlink;
use std::sync::Arc;

const TAG: &str = "0123456789abcdef";

fn contents_name(byte: u8) -> String {
    format!("{TAG}.{}.json", hex_key(byte))
}

/// The plan's refusal for a survey, which is what a person would meet.
fn refusal(surveyed: &Surveyed) -> Option<Refusal> {
    plan_of(surveyed).err()
}

/// A state where one corpus has a live agent folder as well as its set-aside one.
fn with_live_agent(w: &World, key: CorpusKey) {
    let claim = corpus::claim_agent_folder(&w.corpus, key).expect("claimed");
    drop(claim);
}

/// Failure: removal still refuses on another document's Find text (the per-document survey's "can't be proved another's is `Unknown`"), or leaves part of the
/// cache out of the plan: a note's or contents file's temp, a set-aside agent folder, a live one. Everything the app writes, for both documents, is `Target`
/// (owner, 2026-11-27: "Yes, clear them all"), the four stores are complete, and a real plan is made.
#[test]
fn clear_all_plans_every_documents_caches() {
    let (w, mine, theirs) = two_documents();
    with_live_agent(&w, theirs);
    let note = fs::read_dir(w.corpus.join(names::VERSIONS))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name()
        .into_string()
        .unwrap();
    plain(&w.corpus.join(names::VERSIONS).join(format!("{note}.tmp77")));
    w.kept(&format!("{TAG}.{}.tmp9", hex_key(9)));
    let (surveyed, pins) = w.clear_survey(&CacheCaps::default()).unwrap();
    let seen = Seen::of(&surveyed);
    for store in OURS {
        assert!(seen.complete(store), "{store:?}");
    }
    for key in [mine, theirs] {
        assert_eq!(seen.owner(Store::CorpusText, &key.hex()), Owner::Target);
        assert_eq!(
            seen.owner(
                Store::GeneratedContents,
                &format!("{}.{}.json", names::contents_tag(w.mine.as_os_str().as_encoded_bytes()), mine.hex())
            ),
            Owner::Target
        );
    }
    assert!(seen.count(Store::CorpusAgentPages) >= 3, "each corpus's set-aside folder and the live one");
    assert!(seen.count(Store::CorpusAliases) >= 5, "both documents' two notes and a temp");
    assert_eq!(seen.owner(Store::CorpusAliases, &format!("by-file/{note}.tmp77")), Owner::Target);
    assert_eq!(seen.owner(Store::GeneratedContents, &format!("{TAG}.{}.tmp9", hex_key(9))), Owner::Target);
    for (artifact, _) in seen.all() {
        assert_eq!(artifact.owner, Owner::Target, "{artifact:?}");
        assert_eq!(artifact.format, Format::Current, "a note is never opened, so never judged");
    }
    assert_eq!(
        pins.get(&ArtifactKey::of(Store::CorpusText, theirs.hex().as_bytes())).map(|id| id.kind),
        Some(Kind::Folder)
    );
    assert_eq!(refusal(&surveyed), None, "a plan is made");
}

/// Failure: something the app never writes, or not as a plain thing of ours, is taken as ours and cleared (a link followed to the person's file, a pipe waited on,
/// a folder where a file belongs, a name with another link, an uppercase key, a stray), or passed over so the removal doesn't refuse. Each form refuses the plan,
/// naming its store, and is never `Target`.
#[test]
fn under_clear_all_links_pipes_and_strays_still_refuse() {
    type Make = Box<dyn Fn(&World)>;
    let at_root = |name: String, make: fn(&Path)| -> Make { Box::new(move |w| make(&w.corpus.join(&name))) };
    let in_notes = |name: String, make: fn(&Path)| -> Make { Box::new(move |w| make(&w.corpus.join(names::VERSIONS).join(&name))) };
    let in_contents = |name: String, make: fn(&Path)| -> Make { Box::new(move |w| make(&w.contents.join(&name))) };
    let to_persons_file: fn(&Path) = |p| {
        dir(p.parent().unwrap());
        symlink("/etc/hosts", p).unwrap();
    };
    let folder: fn(&Path) = dir;
    let (key, note, kept) = (hex_key(0xa1), hex_key(0xa2), contents_name(0xa3));
    let upper = hex_key(0xa4).to_uppercase();
    let cases: Vec<(&str, Store, Make)> = vec![
        (
            "a key as a link to a folder",
            Store::CorpusText,
            at_root(key.clone(), |p| {
                dir(p.parent().unwrap());
                let target = p.with_file_name("elsewhere");
                fs::create_dir_all(&target).unwrap();
                symlink(&target, p).unwrap();
            }),
        ),
        ("a key as a pipe", Store::CorpusText, at_root(key.clone(), pipe)),
        ("a key as a file", Store::CorpusText, at_root(key.clone(), plain)),
        ("a note as a link", Store::CorpusAliases, in_notes(note.clone(), to_persons_file)),
        ("a note as a pipe", Store::CorpusAliases, in_notes(note.clone(), pipe)),
        ("a note as a folder", Store::CorpusAliases, in_notes(note.clone(), folder)),
        ("contents as a link", Store::GeneratedContents, in_contents(kept.clone(), to_persons_file)),
        ("contents as a pipe", Store::GeneratedContents, in_contents(kept.clone(), pipe)),
        ("contents as a folder", Store::GeneratedContents, in_contents(kept.clone(), folder)),
        ("uppercase hex", Store::CorpusText, at_root(upper, folder)),
        ("a stray in the root", Store::CorpusText, at_root("notes.txt".into(), plain)),
        ("by-file's temp name", Store::CorpusText, at_root("by-file.tmp1".into(), plain)),
        ("sealed.bak", Store::CorpusText, at_root("sealed.bak".into(), plain)),
        ("a stray note", Store::CorpusAliases, in_notes("notes.txt".into(), plain)),
        ("a stray in contents", Store::GeneratedContents, in_contents("notes.txt".into(), plain)),
        (
            "old/corpus as a link",
            Store::CorpusText,
            Box::new(|w| {
                symlink("/etc", w.dir.join("corpus")).unwrap();
            }),
        ),
        ("old/corpus as a file", Store::CorpusText, Box::new(|w| plain(&w.dir.join("corpus")))),
    ];
    for (what, store, make) in cases {
        let w = World::new();
        dir(&w.corpus);
        make(&w);
        let (surveyed, pins) = w.clear_survey(&CacheCaps::default()).unwrap();
        let found = Seen::of(&surveyed);
        assert!(
            found.all().iter().all(|(a, k)| a.owner != Owner::Target && pins.get(k).is_none()),
            "{what}: nothing is taken"
        );
        assert_eq!(refusal(&surveyed), Some(Refusal::Incomplete(store)), "{what}");
    }
    // A name with another link is shared, which refuses too; the file keeps its other name.
    for (what, store, make) in [
        ("a note with two links", Store::CorpusAliases, in_notes(note, two_links as fn(&Path))),
        ("contents with two links", Store::GeneratedContents, in_contents(kept, two_links as fn(&Path))),
    ] {
        let w = World::new();
        dir(&w.corpus);
        make(&w);
        let (surveyed, _) = w.clear_survey(&CacheCaps::default()).unwrap();
        let found = Seen::of(&surveyed);
        assert_eq!(
            found.all().iter().filter(|(a, _)| a.owner == Owner::Shared && a.store == store).count(),
            1,
            "{what}"
        );
        assert!(refusal(&surveyed).is_some(), "{what}");
    }
}

/// Failure: a corpus whose tree holds something that isn't a plain file of ours (a link a person left in it, a pipe, a second name for a file outside, another
/// user's file) is cleared with its tree, so what it leads to is taken or the walk waits. That corpus is `Unknown` and the plan refuses, whichever of the two
/// subfolders the thing is in, while a clean corpus beside it is still the target's.
#[test]
fn a_corpus_holding_a_link_pipe_second_link_or_another_users_file_refuses() {
    let persons = |w: &World| {
        let file = w.dir.join("persons-file");
        fs::write(&file, b"mine").unwrap();
        file
    };
    type Plant = fn(&Path, &Path);
    let plants: [(&str, Plant); 4] = [
        ("a link", |at, file| symlink(file, at).unwrap()),
        ("a pipe", |at, _| crate::removal_store::testing::mkfifo(at)),
        ("a second name", |at, file| fs::hard_link(file, at).unwrap()),
        ("another user's file", |at, _| fs::write(at, b"theirs").unwrap()),
    ];
    for inside in ["app", "agent/pages"] {
        for (what, plant) in plants {
            let w = World::new();
            let (bad, good) = (hex_key(0xb1), hex_key(0xb2));
            let (bad_dir, _) = (w.key_folder(&bad), w.key_folder(&good));
            let planted = bad_dir.join("v4").join(inside);
            dir(&planted);
            let file = persons(&w);
            plant(&planted.join("planted"), &file);
            let mut caps = CacheCaps::default();
            if what == "another user's file" {
                let other = recto_platform::local::current_uid().wrapping_add(1);
                caps.uid_of = Some(Arc::new(move |name: &OsStr| (name == "planted").then_some(other)));
            }
            let (surveyed, pins) = w.clear_survey(&caps).unwrap();
            let found = Seen::of(&surveyed);
            let case = format!("{what} in {inside}");
            assert_eq!(found.owner(Store::CorpusText, &bad), Owner::Unknown, "{case}");
            assert_eq!(found.owner(Store::CorpusText, &good), Owner::Target, "{case}");
            assert!(pins.get(&ArtifactKey::of(Store::CorpusText, bad.as_bytes())).is_none(), "{case}");
            assert_eq!(refusal(&surveyed), Some(Refusal::Incomplete(Store::CorpusText)), "{case}");
            assert_eq!(fs::read(&file).unwrap(), b"mine", "{case}: the person's file is untouched");
        }
    }
}

/// Failure: a pin is made for something that isn't taken (a stray, a shared note, a link), or an artifact the purge will take has no pin or one of the wrong
/// kind, so the act refuses what it should do or does what it was never shown. Every `Target` has a pin of its store's kind (a folder for the two folder stores, a
/// file for the other two), and no other owner has one.
#[test]
fn pins_are_exactly_the_purgeable_entries() {
    let (w, mine, theirs) = two_documents();
    with_live_agent(&w, mine);
    let shared = w.corpus.join(names::VERSIONS).join(hex_key(0xc1));
    two_links(&shared);
    link(&w.corpus.join(hex_key(0xc2)));
    plain(&w.corpus.join("stray"));
    w.kept(&contents_name(0xc3));
    pipe(&w.contents.join(contents_name(0xc4)));
    let _ = theirs;
    let (surveyed, pins) = w.clear_survey(&CacheCaps::default()).unwrap();
    let found = Seen::of(&surveyed);
    let (mut targets, mut others) = (0, 0);
    for (artifact, key) in found.all() {
        let pin = pins.get(key);
        if artifact.owner == Owner::Target {
            targets += 1;
            let want = if matches!(artifact.store, Store::CorpusText | Store::CorpusAgentPages) {
                Kind::Folder
            } else {
                Kind::File
            };
            assert_eq!(pin.map(|id| id.kind), Some(want), "{artifact:?}");
        } else {
            others += 1;
            assert!(pin.is_none(), "{artifact:?} is not taken and has no pin");
        }
    }
    assert!(targets >= 8 && others >= 4, "{targets} taken, {others} refused: the fixture holds both");
}

/// Failure: the clear-all reuses the plan's listing, so a corpus a build made after the plan, or in a root that was missing then, is not seen, and the carrier
/// reports the caches clear while the words are on disk.
#[test]
fn caches_are_clear_sees_what_came_after_the_plan() {
    let w = World::new();
    assert_eq!(caches_are_clear(&w.corpus, &w.contents), Ok(true), "roots that were never made are clear");
    let key = w.index(&w.mine);
    assert_eq!(
        caches_are_clear(&w.corpus, &w.contents),
        Ok(false),
        "a corpus made by a real build after the plan"
    );
    w.keep_contents(&w.mine, key);
    corpus::clear_all(&w.corpus).unwrap();
    assert_eq!(caches_are_clear(&w.corpus, &w.contents), Ok(false), "contents left");
    recto_index::generated::clear_all(&w.contents);
    assert_eq!(
        caches_are_clear(&w.corpus, &w.contents),
        Ok(true),
        "the app's own clear leaves nothing the survey finds"
    );
    plain(&w.corpus.join("stray"));
    assert_eq!(caches_are_clear(&w.corpus, &w.contents), Ok(false), "a stray is something found");
}

/// Failure: the tree's cap counts wrong, so a corpus of exactly the cap is refused or one over it is passed: one entry over is `TooLarge`, at it is fine, and a
/// refused entry is not mistaken for the cap.
#[test]
fn a_corpus_tree_holds_the_cap_and_one_more_is_too_large() {
    let w = World::new();
    let key = hex_key(0xd1);
    let folder = w.key_folder(&key);
    // `v4`, `app` and `text.pack` are three entries; two more make five.
    plain(&folder.join("v4").join("app").join("one"));
    plain(&folder.join("v4").join("app").join("two"));
    let at = small(|c| c.tree_entries = 5);
    assert_eq!(
        Seen::of(&w.clear_survey(&at).unwrap().0).owner(Store::CorpusText, &key),
        Owner::Target,
        "at the cap"
    );
    plain(&folder.join("v4").join("app").join("three"));
    assert_eq!(w.clear_survey(&at).err(), Some(Refusal::TooLarge), "one over");
}
