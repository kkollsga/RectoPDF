//! The caches survey over real folders: what the app's own writers leave, and what a hostile folder can hold. Each test names the failure it catches.

use super::*;
use crate::removal_store::testing::{limits, mkfifo, version};
use recto_core::removal::{Artifact, LEDGER_LIMITS, Ledger, Link, StoreSet, Surveyed, lineage};
use recto_index::corpus::{self, BuildLimits, BuildOutcome, DocMeta, FedPage, FeedError, PageFeed};
use recto_platform::find_key::FindKey;
use recto_platform::local;
use recto_proto::CharRecord;
use recto_testpdf::TempDir;
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::symlink;
use std::sync::Arc;

/// A state folder as the app leaves it: two documents on disk, a corpus root named as the app names it (so the older folder's place is where it is), and a contents
/// folder, all under one canonical, private folder.
pub(crate) struct World {
    pub dir: TempDir,
    pub corpus: PathBuf,
    pub contents: PathBuf,
    pub mine: PathBuf,
    pub theirs: PathBuf,
    pub find_key: FindKey,
}

/// One page of text, so a build finishes.
struct OnePage;

impl PageFeed for OnePage {
    fn page_count(&mut self) -> Result<u32, FeedError> {
        Ok(1)
    }
    fn read(&mut self, _page: u32) -> Result<FedPage, FeedError> {
        Ok(FedPage {
            chars: "some words".chars().map(|c| CharRecord::new(c as u32, 0)).collect(),
            boxes: Vec::new(),
            hidden_checked: true,
        })
    }
    fn still_the_same_file(&mut self) -> bool {
        true
    }
}

impl World {
    pub fn new() -> Self {
        let dir = TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-caches-");
        local::ensure_private_dir(&dir).unwrap();
        let docs = dir.join("docs");
        fs::create_dir(&docs).unwrap();
        let (mine, theirs) = (docs.join("mine.pdf"), docs.join("theirs.pdf"));
        fs::write(&mine, b"%PDF-1.4 mine").unwrap();
        fs::write(&theirs, b"%PDF-1.4 theirs, longer").unwrap();
        let find_key = FindKey::from_state_file(&dir.join("find.key")).unwrap();
        Self {
            corpus: dir.join("corpus.noindex"),
            contents: dir.join("contents.noindex"),
            dir,
            mine,
            theirs,
            find_key,
        }
    }

    /// Builds the document's corpus the way the indexer does (which notes its key under the path's and the inode's names), and returns its key.
    pub fn index(&self, path: &Path) -> CorpusKey {
        let key = CorpusKey::of_file(path).unwrap();
        let built = corpus::build_for_file(
            &self.find_key,
            path,
            &self.corpus,
            key,
            &mut OnePage,
            &DocMeta::default(),
            &BuildLimits::default(),
        )
        .unwrap();
        assert!(matches!(built, BuildOutcome::Ready { .. }), "{built:?}");
        key
    }

    /// Keeps the document's generated contents, as the app does.
    pub fn keep_contents(&self, path: &Path, key: CorpusKey) {
        recto_index::generated::write(&self.contents, path, key, &recto_index::generated::Generated::default()).unwrap();
    }

    /// A corpus folder with its text where the app puts it, for a key a test made up.
    pub fn key_folder(&self, hex: &str) -> PathBuf {
        let app = self.corpus.join(hex).join("v4").join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("text.pack"), b"sealed bytes").unwrap();
        self.corpus.join(hex)
    }

    /// A version note, as a plain file of this user's.
    pub fn note(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let folder = self.corpus.join(names::VERSIONS);
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    pub fn kept(&self, name: &str) -> PathBuf {
        fs::create_dir_all(&self.contents).unwrap();
        let path = self.contents.join(name);
        fs::write(&path, b"{}").unwrap();
        path
    }

    pub fn target(&self) -> CacheTarget {
        CacheTarget {
            key: CorpusKey::of_file(&self.mine).ok(),
            device_inode: recto_platform::file_device_and_inode(&self.mine),
        }
    }

    /// This document at its path (inode 7) and the other at its own (inode 8), from the beginning of time.
    pub fn ledger(&self) -> Ledger {
        let at = |inode, path: &Path| Link::At {
            version: version(inode),
            path: path.to_path_buf(),
        };
        Ledger {
            links: vec![at(7, &self.mine), at(8, &self.theirs)],
            destinations: Vec::new(),
            path_bytes: 0,
            began_ns: 0,
            gap: None,
            stopped: None,
            good_len: 0,
            frames: 0,
            overflowed: false,
        }
    }

    /// The caches survey of this state, every other store marked complete.
    pub fn survey(&self, mode: TargetMode, caps: &CacheCaps, max: u32) -> Result<Surveyed, Refusal> {
        self.survey_of(&self.target(), mode, caps, max)
    }

    pub fn survey_of(&self, target: &CacheTarget, mode: TargetMode, caps: &CacheCaps, max: u32) -> Result<Surveyed, Refusal> {
        let ledger = self.ledger();
        let lineage = lineage(&ledger, version(7), &self.mine, &LEDGER_LIMITS).unwrap();
        let mut survey = Survey::new(&limits(max));
        for store in Store::ALL.into_iter().filter(|s| s.enumerated()) {
            if !OURS.contains(&store) {
                survey.mark_complete(store);
            }
        }
        survey.history(&lineage);
        survey_caches_with(&self.corpus, &self.contents, &lineage, target, mode, &mut survey, caps)?;
        survey.finish()
    }

    pub fn seen(&self, mode: TargetMode) -> Seen {
        Seen::of(&self.survey(mode, &CacheCaps::default(), 4096).unwrap())
    }
}

/// The four stores this adapter owns.
pub(crate) const OURS: [Store; 4] = [Store::CorpusText, Store::CorpusAgentPages, Store::CorpusAliases, Store::GeneratedContents];

/// What a survey found, by store and name.
pub(crate) struct Seen {
    found: Vec<(Artifact, ArtifactKey)>,
    complete: StoreSet,
}

impl Seen {
    pub fn of(surveyed: &Surveyed) -> Self {
        let inventory = surveyed.inventory();
        let found = inventory
            .artifacts()
            .iter()
            .map(|a| (*a, surveyed.key(a.id).expect("every artifact has a key").1))
            .collect();
        Self {
            found,
            complete: inventory.complete(),
        }
    }

    pub fn complete(&self, store: Store) -> bool {
        self.complete.contains(store)
    }

    /// The owner and format of the artifact of `store` whose root-relative name is `name`.
    pub fn get(&self, store: Store, name: &str) -> Option<(Owner, Format)> {
        let key = ArtifactKey::of(store, name.as_bytes());
        self.found.iter().find(|(a, k)| a.store == store && *k == key).map(|(a, _)| (a.owner, a.format))
    }

    pub fn owner(&self, store: Store, name: &str) -> Owner {
        self.get(store, name).unwrap_or_else(|| panic!("no {store:?} named {name:?}")).0
    }

    pub fn count(&self, store: Store) -> usize {
        self.found.iter().filter(|(a, _)| a.store == store).count()
    }

    /// The keys of the artifacts of `store` that are `owner`'s.
    pub fn keys_of(&self, store: Store, owner: Owner) -> BTreeSet<ArtifactKey> {
        self.found
            .iter()
            .filter(|(a, _)| a.store == store && a.owner == owner)
            .map(|(_, k)| *k)
            .collect()
    }
}

fn hex_key(byte: u8) -> String {
    format!("{byte:02x}").repeat(32)
}

fn names_of(store: Store, names: impl IntoIterator<Item = String>) -> BTreeSet<ArtifactKey> {
    names.into_iter().map(|name| ArtifactKey::of(store, name.as_bytes())).collect()
}

fn small(change: impl FnOnce(&mut CacheCaps)) -> CacheCaps {
    let mut caps = CacheCaps::default();
    change(&mut caps);
    caps
}

/// A World with both documents indexed, each with contents and an agent's folder set aside: a state with two documents' material in it.
fn two_documents() -> (World, CorpusKey, CorpusKey) {
    let w = World::new();
    let (mine, theirs) = (w.index(&w.mine), w.index(&w.theirs));
    w.keep_contents(&w.mine, mine);
    w.keep_contents(&w.theirs, theirs);
    for key in [mine, theirs] {
        let claim = corpus::claim_agent_folder(&w.corpus, key).expect("the folder is claimed");
        corpus::retire_agent_folder(&w.corpus, key, claim);
    }
    (w, mine, theirs)
}

/// Failure: removal clears less than the app's own `index clear <file>` would (the document's old versions' text stays behind), or more (another document's text
/// goes). The corpora the survey calls the target's are the corpora `keys_of_file` names that are on disk, and the contents it calls the target's are the files
/// `generated::clear` removes from a copy of the folder. Seen failing with each of: the path's alias hashing another path, the inode's alias left out, and the
/// aliases of every lineage path claimed.
#[test]
fn the_targets_set_is_what_index_clear_removes() {
    let (w, mine, theirs) = two_documents();
    // A version of this file from before it was saved over, which only the inode's note still names (the key of a replaced file can't be made again).
    let old = hex_key(0xee);
    w.key_folder(&old);
    let inode_note = names::inode_alias(w.target().device_inode.unwrap().0, w.target().device_inode.unwrap().1);
    let note = w.corpus.join(names::VERSIONS).join(&inode_note);
    let kept = fs::read_to_string(&note).unwrap();
    fs::write(&note, format!("{old}\n{kept}")).unwrap();

    let seen = w.seen(TargetMode::Save);
    let on_disk: BTreeSet<String> = corpus::keys_of_file(&w.corpus, &w.mine)
        .into_iter()
        .map(|key| key.hex())
        .filter(|hex| w.corpus.join(hex).is_dir())
        .collect();
    assert!(on_disk.contains(&old) && on_disk.contains(&mine.hex()) && !on_disk.contains(&theirs.hex()));
    assert_eq!(
        seen.keys_of(Store::CorpusText, Owner::Target),
        names_of(Store::CorpusText, on_disk),
        "the corpora that are the target's"
    );

    // What the app's clear removes, from a copy of the folder.
    let copy = w.dir.join("contents-copy");
    fs::create_dir(&copy).unwrap();
    let listed = |folder: &Path| -> BTreeSet<String> { fs::read_dir(folder).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect() };
    for name in listed(&w.contents) {
        fs::copy(w.contents.join(&name), copy.join(&name)).unwrap();
    }
    let before = listed(&copy);
    recto_index::generated::clear(&copy, &w.mine);
    let removed: BTreeSet<String> = before.difference(&listed(&copy)).cloned().collect();
    assert_eq!(removed.len(), 1);
    assert_eq!(
        seen.keys_of(Store::GeneratedContents, Owner::Target),
        names_of(Store::GeneratedContents, removed),
        "the contents that are the target's"
    );

    // Its notes: the two named for it are its own. Another document's are not.
    let own = [names::path_alias(w.mine.as_os_str().as_encoded_bytes()), inode_note];
    for note in own {
        assert_eq!(seen.owner(Store::CorpusAliases, &format!("by-file/{note}")), Owner::Target);
    }
    let theirs_note = names::path_alias(w.theirs.as_os_str().as_encoded_bytes());
    assert_eq!(seen.owner(Store::CorpusAliases, &format!("by-file/{theirs_note}")), Owner::Unknown);
}

impl World {
    /// The names of the agent's folders (live or set aside) in a corpus's `v4` folder.
    pub fn agent_names(&self, key: CorpusKey) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.corpus.join(key.hex()).join("v4"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.starts_with("agent"))
            .collect();
        names.sort();
        names
    }
}

/// Failure: another document's caches are called the target's or the Save As source's, so removing one document clears or keeps the other's text. Both
/// documents' material is in the state; whatever the mode, none of the other's is placed (its corpus, its agent's folder, its contents), and a note named for the
/// other's path, which the lineage knows, is not the target's. Seen failing with the notes of every lineage path claimed.
#[test]
fn another_documents_caches_are_never_the_targets() {
    let (w, mine, theirs) = two_documents();
    let theirs_agents = w.agent_names(theirs);
    assert_eq!(theirs_agents.len(), 1, "an agent's folder set aside is there to be found");
    let theirs_contents: Vec<String> = fs::read_dir(&w.contents)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.contains(&theirs.hex()))
        .collect();
    assert_eq!(theirs_contents.len(), 1);
    for mode in [TargetMode::Save, TargetMode::SaveAs] {
        let seen = w.seen(mode);
        let placed = |store: Store, name: &str| matches!(seen.owner(store, name), Owner::Target | Owner::SaveAsSource);
        assert!(placed(Store::CorpusText, &mine.hex()), "{mode:?}: its own is placed");
        assert!(!placed(Store::CorpusText, &theirs.hex()), "{mode:?}");
        assert!(
            !placed(Store::CorpusAgentPages, &format!("{}/v4/{}", theirs.hex(), theirs_agents[0])),
            "{mode:?}"
        );
        assert!(!placed(Store::GeneratedContents, &theirs_contents[0]), "{mode:?}");
        let theirs_note = names::path_alias(w.theirs.as_os_str().as_encoded_bytes());
        assert!(!placed(Store::CorpusAliases, &format!("by-file/{theirs_note}")), "{mode:?}");
        // Nothing in the caches can be proved another document's, so nothing is `Unrelated`: it is `Unknown`, and a plan refuses.
        assert!(seen.found.iter().all(|(a, _)| a.owner != Owner::Unrelated), "{mode:?}");
    }
}

/// Failure: a Save As keeps the wrong thing. The target's artifacts are the source's (kept, never acted on) and nothing else is placed.
#[test]
fn a_save_as_places_the_targets_as_the_source() {
    let (w, mine, _theirs) = two_documents();
    let save = w.seen(TargetMode::Save);
    let save_as = w.seen(TargetMode::SaveAs);
    for store in OURS {
        assert_eq!(save.count(store), save_as.count(store), "{store:?}");
        assert_eq!(save.keys_of(store, Owner::Target), save_as.keys_of(store, Owner::SaveAsSource), "{store:?}");
        assert!(save_as.keys_of(store, Owner::Target).is_empty(), "{store:?}");
    }
    assert!(!save_as.keys_of(Store::CorpusText, Owner::SaveAsSource).is_empty());
    assert_eq!(save_as.owner(Store::CorpusText, &mine.hex()), Owner::SaveAsSource);
}

/// One name in one folder, made by `make` in a fresh state, and what the survey says of it: `None` for left out of the survey. Run under a bound, so a pipe
/// the survey waited on for a writer fails the test instead of hanging the run.
fn expect(what: &str, make: impl FnOnce(&World), store: Store, name: &str, want: Option<(Owner, Format)>) {
    let w = World::new();
    make(&w);
    let name = name.to_string();
    let seen = recto_testpdf::returns_in_time(what, move || w.seen(TargetMode::Save).get(store, &name));
    assert_eq!(seen, want, "{what}");
}

fn dir(path: &Path) {
    fs::create_dir_all(path).unwrap();
}

fn plain(path: &Path) {
    dir(path.parent().unwrap());
    fs::write(path, b"x").unwrap();
}

fn link(path: &Path) {
    dir(path.parent().unwrap());
    symlink("/nonexistent-target", path).unwrap();
}

fn pipe(path: &Path) {
    dir(path.parent().unwrap());
    mkfifo(path);
}

/// A second name for `path`, which is made as a plain file first.
fn two_links(path: &Path) {
    plain(path);
    fs::hard_link(path, path.with_file_name("a-second-name")).unwrap();
}

const UNKNOWN: Option<(Owner, Format)> = Some((Owner::Unknown, Format::Current));

/// Failure: an entry of the corpus root nobody can place is skipped, or read through (a pipe waited on, a link followed), so a removal passes over material it
/// can't name, or hangs. Each form of a corpus's name that the app doesn't write, and each wrong form of its markers, is `Unknown`; Finder's own file is left out.
#[test]
fn a_corpus_root_holds_only_what_the_app_writes_or_is_unknown() {
    let k = |byte| hex_key(byte);
    let upper = hex_key(0xab).to_uppercase();
    type Make = fn(&Path);
    let cases: Vec<(&str, String, Make, Option<(Owner, Format)>)> = vec![
        ("uppercase hex", upper.clone(), |p| dir(p), UNKNOWN),
        ("a key as a file", k(1), plain, UNKNOWN),
        ("a key as a link", k(2), link, UNKNOWN),
        ("a key as a pipe", k(3), pipe, UNKNOWN),
        ("a key's temp", format!("{}.tmp1", k(4)), |p| dir(p), UNKNOWN),
        ("a stray file", "stray".into(), plain, UNKNOWN),
        ("a stray folder", "stray".into(), |p| dir(p), UNKNOWN),
        ("sealed as a plain file", "sealed".into(), plain, None),
        ("sealed's temp", "sealed.tmp12".into(), plain, None),
        ("sealed as a folder", "sealed".into(), |p| dir(p), UNKNOWN),
        ("sealed with two links", "sealed".into(), two_links, UNKNOWN),
        ("sealed as a link", "sealed".into(), link, UNKNOWN),
        ("sealed as a pipe", "sealed".into(), pipe, UNKNOWN),
        ("the agent mark as a plain file", "agent-folders".into(), plain, None),
        ("the agent mark's temp", "agent-folders.tmp3".into(), plain, None),
        ("the agent mark as a folder", "agent-folders".into(), |p| dir(p), UNKNOWN),
        (".DS_Store as a plain file", ".DS_Store".into(), plain, None),
        (
            ".DS_Store with two links",
            ".DS_Store".into(),
            two_links,
            Some((Owner::Shared, Format::Current)),
        ),
        (".DS_Store as a folder", ".DS_Store".into(), |p| dir(p), UNKNOWN),
        (".DS_Store as a link", ".DS_Store".into(), link, UNKNOWN),
    ];
    for (what, name, make, want) in cases {
        expect(what, |w| make(&w.corpus.join(&name)), Store::CorpusText, &name, want);
    }
}

/// Failure: an old build's folder of plain text is skipped (its words stay when the document is removed), or read through its link.
#[test]
fn the_old_plain_text_folder_is_unknown_in_any_form() {
    type Make = fn(&Path);
    let cases: [(&str, Make); 3] = [("a folder", |p| dir(p)), ("a file", plain), ("a link", link)];
    for (what, make) in cases {
        expect(what, |w| make(&w.dir.join("corpus")), Store::CorpusText, "old/corpus", UNKNOWN);
    }
    expect("none there", |_| {}, Store::CorpusText, "old/corpus", None);
    // Only beside a corpus root of the name the app gives it.
    let w = World::new();
    plain(&w.dir.join("corpus"));
    let mut elsewhere = World::new();
    elsewhere.corpus = elsewhere.dir.join("other-name");
    plain(&elsewhere.dir.join("corpus"));
    assert!(
        Seen::of(&w.survey(TargetMode::Save, &CacheCaps::default(), 64).unwrap())
            .get(Store::CorpusText, "old/corpus")
            .is_some()
    );
    assert!(
        Seen::of(&elsewhere.survey(TargetMode::Save, &CacheCaps::default(), 64).unwrap())
            .get(Store::CorpusText, "old/corpus")
            .is_none()
    );
}

/// Failure: a note the survey can't trust is skipped or its keys believed. A pipe is never waited on; a placed note with other names is shared; a note that is not
/// exactly what the app writes is `Foreign`, so a later rewrite of it is refused, rather than rewritten as if it were understood.
#[test]
fn a_version_note_is_placed_by_its_name_and_read_strictly() {
    let mine = |w: &World| names::path_alias(w.mine.as_os_str().as_encoded_bytes());
    let note = |w: &World| format!("by-file/{}", mine(w));
    let (a, b) = (hex_key(1), hex_key(2));
    let many = |n: u8| (0..n).map(hex_key).collect::<Vec<_>>().join("\n");
    let wrote = |bytes: Vec<u8>| move |w: &World| drop(w.note(&mine(w), &bytes));
    let foreign: Vec<(&str, Vec<u8>)> = vec![
        ("33 keys", many(33).into_bytes()),
        ("a bad line", format!("{a}\nnot a key").into_bytes()),
        ("crlf", format!("{a}\r\n{b}").into_bytes()),
        ("a trailing line break", format!("{a}\n").into_bytes()),
        ("a duplicate", format!("{a}\n{a}").into_bytes()),
        ("one byte over the cap", vec![b'a'; names::ALIAS_BYTES + 1]),
        ("empty", Vec::new()),
    ];
    for (what, bytes) in foreign {
        let w = World::new();
        wrote(bytes)(&w);
        let seen = w.seen(TargetMode::Save);
        assert_eq!(seen.get(Store::CorpusAliases, &note(&w)), Some((Owner::Target, Format::Foreign)), "{what}");
    }
    let w = World::new();
    wrote(many(32).into_bytes())(&w);
    assert_eq!(
        w.seen(TargetMode::Save).get(Store::CorpusAliases, &note(&w)),
        Some((Owner::Target, Format::Current)),
        "32 keys are the most"
    );

    let w = World::new();
    let path = w.note(&mine(&w), a.as_bytes());
    fs::hard_link(&path, path.with_file_name("zz")).unwrap();
    let seen = w.seen(TargetMode::Save);
    assert_eq!(seen.owner(Store::CorpusAliases, &note(&w)), Owner::Shared, "a placed note with another name");
    assert_eq!(seen.owner(Store::CorpusAliases, "by-file/zz"), Owner::Unknown, "a name nobody writes");

    let w = World::new();
    let temp = format!("by-file/{}.tmp77", mine(&w));
    w.note(&format!("{}.tmp77", mine(&w)), a.as_bytes());
    assert_eq!(
        w.seen(TargetMode::Save).owner(Store::CorpusAliases, &temp),
        Owner::Target,
        "a write's temp is placed like the note"
    );

    let w = World::new();
    pipe(&w.corpus.join("by-file").join(mine(&w)));
    let name = note(&w);
    let owner = recto_testpdf::returns_in_time("a pipe as a note", move || w.seen(TargetMode::Save).owner(Store::CorpusAliases, &name));
    assert_eq!(owner, Owner::Unknown);
}

/// Failure: a note of another document's, which lists no key of the target's, is placed; or a note that lists one of the target's keys under another document's
/// name (an old version's, named for a path it had before) is left. A note is the target's by its name or by what it lists, as the app's own clear takes it.
#[test]
fn a_note_is_placed_by_its_name_or_by_a_key_the_target_has() {
    let w = World::new();
    let key = CorpusKey::of_file(&w.mine).unwrap().hex();
    w.note(&"c".repeat(64), key.as_bytes());
    w.note(&"d".repeat(64), hex_key(9).as_bytes());
    let seen = w.seen(TargetMode::Save);
    assert_eq!(seen.owner(Store::CorpusAliases, &format!("by-file/{}", "c".repeat(64))), Owner::Target);
    assert_eq!(seen.owner(Store::CorpusAliases, &format!("by-file/{}", "d".repeat(64))), Owner::Unknown);
}

/// Failure: a contents file is placed by something a stranger can make (any name that looks right), or one that is the target's is left. Placed by the current
/// path's tag or a key the target has; every wrong form is `Unknown`, and a placed file with other names is shared.
#[test]
fn a_contents_file_is_placed_by_its_tag_or_its_key() {
    let w = World::new();
    let tag = names::contents_tag(w.mine.as_os_str().as_encoded_bytes());
    let (other_tag, key, other_key) = ("0123456789abcdef", hex_key(5), hex_key(6));
    let own = format!("{tag}.{key}.json");
    let theirs = format!("{other_tag}.{other_key}.json");
    let by_key = format!("{other_tag}.{}.json", CorpusKey::of_file(&w.mine).unwrap().hex());
    let temp = format!("{tag}.{key}.tmp9");
    for name in [&own, &theirs, &by_key, &temp, &"stray".to_string()] {
        w.kept(name);
    }
    let seen = w.seen(TargetMode::Save);
    assert_eq!(seen.owner(Store::GeneratedContents, &own), Owner::Target);
    assert_eq!(seen.owner(Store::GeneratedContents, &temp), Owner::Target);
    assert_eq!(
        seen.owner(Store::GeneratedContents, &by_key),
        Owner::Target,
        "a version of this file under an older path's tag"
    );
    assert_eq!(seen.owner(Store::GeneratedContents, &theirs), Owner::Unknown);
    assert_eq!(seen.owner(Store::GeneratedContents, "stray"), Owner::Unknown);

    // Each hostile form under a name of its own, in this state, surveyed once under a bound (a pipe is never waited on).
    let named = |byte: u8| format!("{tag}.{}.json", hex_key(byte));
    let upper = format!("{}.{}.json", tag.to_uppercase(), hex_key(0xa1));
    plain(&w.contents.join(&upper));
    two_links(&w.contents.join(named(0xa2)));
    link(&w.contents.join(named(0xa3)));
    dir(&w.contents.join(named(0xa4)));
    pipe(&w.contents.join(named(0xa5)));
    plain(&w.contents.join(".DS_Store"));
    let (names, owners): (Vec<String>, Vec<Option<(Owner, Format)>>) = [
        (upper, UNKNOWN),
        (named(0xa2), Some((Owner::Shared, Format::Current))),
        (named(0xa3), UNKNOWN),
        (named(0xa4), UNKNOWN),
        (named(0xa5), UNKNOWN),
        (".DS_Store".to_string(), None),
    ]
    .into_iter()
    .unzip();
    let wanted = names.clone();
    let got = recto_testpdf::returns_in_time("a contents folder with a pipe in it", move || {
        let seen = w.seen(TargetMode::Save);
        wanted.iter().map(|name| seen.get(Store::GeneratedContents, name)).collect::<Vec<_>>()
    });
    assert_eq!(
        got, owners,
        "an uppercase tag, two links, a link, a folder, a pipe, and Finder's own file: {names:?}"
    );
}

/// Failure: an agent's folder is looked through (its pages opened) or taken for the corpus's own, or one that is a link is followed. Each `agent` and
/// `agent.gone.*` entry of a format folder is one artifact, never opened, owned as its corpus is when it is a folder and `Unknown` when anything else.
#[test]
fn an_agents_folder_is_found_by_name_and_never_opened() {
    let (w, mine, theirs) = two_documents();
    let agent = &w.agent_names(mine)[0];
    let seen = w.seen(TargetMode::Save);
    assert_eq!(seen.owner(Store::CorpusAgentPages, &format!("{}/v4/{agent}", mine.hex())), Owner::Target);
    assert_eq!(
        seen.owner(Store::CorpusAgentPages, &format!("{}/v4/{}", theirs.hex(), w.agent_names(theirs)[0])),
        Owner::Unknown
    );
    assert_eq!(seen.count(Store::CorpusAgentPages), 2, "nothing else in a corpus is an agent's folder");

    let live = w.corpus.join(mine.hex()).join("v4").join("agent");
    link(&live);
    let swapped = w.corpus.join(theirs.hex()).join("v4").join("agent");
    plain(&swapped);
    let seen = w.seen(TargetMode::Save);
    assert_eq!(
        seen.owner(Store::CorpusAgentPages, &format!("{}/v4/agent", mine.hex())),
        Owner::Unknown,
        "a link in place of the folder"
    );
    assert_eq!(
        seen.owner(Store::CorpusAgentPages, &format!("{}/v4/agent", theirs.hex())),
        Owner::Unknown,
        "a file in place of the folder"
    );
}

fn root_is(w: &World) -> PathBuf {
    w.corpus.clone()
}

/// Failure: a folder that is a link, or one swapped after the check, is followed or read as "nothing there", so a removal passes with material in it. A linked
/// root leaves its stores unmarked and a link elsewhere leaves only its own.
#[test]
fn a_linked_folder_is_never_read_and_leaves_its_store_unmarked() {
    let marks = |w: &World| {
        let seen = w.seen(TargetMode::Save);
        OURS.map(|store| seen.complete(store))
    };
    let (w, ..) = two_documents();
    assert_eq!(marks(&w), [true; 4], "a state as the app leaves it is complete");

    let (w, ..) = two_documents();
    let real = w.dir.join("real-corpus");
    fs::rename(&w.corpus, &real).unwrap();
    symlink(&real, &w.corpus).unwrap();
    assert_eq!(marks(&w), [false, false, false, true], "a linked corpus root");

    let (w, ..) = two_documents();
    let real = w.dir.join("real-by-file");
    fs::rename(w.corpus.join("by-file"), &real).unwrap();
    symlink(&real, w.corpus.join("by-file")).unwrap();
    assert_eq!(marks(&w), [true, true, false, true], "a linked by-file");

    let (w, ..) = two_documents();
    let real = w.dir.join("real-contents");
    fs::rename(&w.contents, &real).unwrap();
    symlink(&real, &w.contents).unwrap();
    assert_eq!(marks(&w), [true, true, true, false], "a linked contents folder");
}

/// Failure: the corpus root is listed by its path after it was checked, so a link put in its place after the check is followed (a trap into another folder), or a
/// key folder is opened by path and a canary behind a link is read as the corpus. Both are reached by descriptor, so the swap changes nothing.
#[test]
fn a_folder_swapped_after_it_was_checked_is_still_the_one_that_was_opened() {
    let (w, mine, _theirs) = two_documents();
    let (root, moved, canary) = (root_is(&w), w.dir.join("moved-root"), w.dir.join("canary"));
    dir(&canary.join("v4").join("agent"));
    let hook_root: Hook = {
        let (root, moved, canary) = (root.clone(), moved.clone(), canary.clone());
        Arc::new(move |at: &Path| {
            if at == root {
                fs::rename(&root, &moved).unwrap();
                symlink(&canary, &root).unwrap();
            }
        })
    };
    let caps = small(|c| c.before_list = Some(hook_root));
    let seen = Seen::of(&w.survey(TargetMode::Save, &caps, 4096).unwrap());
    assert!(root.is_symlink(), "the hook swapped the root");
    assert_eq!(
        seen.owner(Store::CorpusText, &mine.hex()),
        Owner::Target,
        "the root that was opened is the one that was listed"
    );
    assert!(seen.complete(Store::CorpusText));

    // A key folder swapped for a link to a folder whose `v4/agent` is there to be found.
    let (w, mine, _theirs) = two_documents();
    let (folder, moved, canary) = (w.corpus.join(mine.hex()), w.dir.join("moved-key"), w.dir.join("canary"));
    dir(&canary.join("v4").join("agent"));
    let hook_open: Hook = {
        let (folder, moved, canary) = (folder.clone(), moved.clone(), canary.clone());
        Arc::new(move |at: &Path| {
            if at == folder {
                fs::rename(&folder, &moved).unwrap();
                symlink(&canary, &folder).unwrap();
            }
        })
    };
    let caps = small(|c| c.before_open = Some(hook_open));
    let seen = Seen::of(&w.survey(TargetMode::Save, &caps, 4096).unwrap());
    assert!(folder.is_symlink());
    assert_eq!(seen.owner(Store::CorpusText, &mine.hex()), Owner::Unknown, "a link is never the target's");
    assert_eq!(
        seen.count(Store::CorpusAgentPages),
        1,
        "only the other document's: nothing from behind the link"
    );
}

/// Failure: a note, a key folder or a format folder that goes away between the listing and its opening is read as "nothing there", so the survey says it saw
/// everything when it did not. Each leaves its stores unmarked.
#[test]
fn something_that_vanishes_between_the_listing_and_its_opening_leaves_its_stores_unmarked() {
    let (w, mine, _theirs) = two_documents();
    let note = w.corpus.join("by-file").join(names::path_alias(w.mine.as_os_str().as_encoded_bytes()));
    let hook: Hook = {
        let note = note.clone();
        Arc::new(move |at: &Path| {
            if at == note {
                let _ = fs::remove_file(&note);
            }
        })
    };
    let seen = Seen::of(&w.survey(TargetMode::Save, &small(|c| c.before_open = Some(hook)), 4096).unwrap());
    assert!(!seen.complete(Store::CorpusAliases), "a note that vanished before it was read");
    assert!(seen.complete(Store::CorpusText));

    let (w, mine, _theirs) = two_documents();
    let key_dir = w.corpus.join(mine.hex());
    let hook: Hook = {
        let key_dir = key_dir.clone();
        Arc::new(move |at: &Path| {
            if at == key_dir {
                fs::remove_dir_all(&key_dir).unwrap();
            }
        })
    };
    let seen = Seen::of(&w.survey(TargetMode::Save, &small(|c| c.before_open = Some(hook)), 4096).unwrap());
    assert!(
        !seen.complete(Store::CorpusText) && !seen.complete(Store::CorpusAgentPages),
        "a key folder that vanished"
    );
    assert!(seen.complete(Store::CorpusAliases));

    let (w, mine, _theirs) = two_documents();
    let format_dir = w.corpus.join(mine.hex()).join("v4");
    let hook: Hook = {
        let format_dir = format_dir.clone();
        Arc::new(move |at: &Path| {
            if at == format_dir {
                fs::remove_dir_all(&format_dir).unwrap();
            }
        })
    };
    let seen = Seen::of(&w.survey(TargetMode::Save, &small(|c| c.before_open = Some(hook)), 4096).unwrap());
    assert!(
        !seen.complete(Store::CorpusText) && !seen.complete(Store::CorpusAgentPages),
        "a format folder that vanished"
    );

    // Swapped for a file rather than gone: only the agent pages can't be said to be whole.
    let (w, mine, _theirs) = two_documents();
    let format_dir = w.corpus.join(mine.hex()).join("v4");
    let hook: Hook = {
        let format_dir = format_dir.clone();
        Arc::new(move |at: &Path| {
            if at == format_dir {
                fs::remove_dir_all(&format_dir).unwrap();
                fs::write(&format_dir, b"x").unwrap();
            }
        })
    };
    let seen = Seen::of(&w.survey(TargetMode::Save, &small(|c| c.before_open = Some(hook)), 4096).unwrap());
    assert!(
        seen.complete(Store::CorpusText) && !seen.complete(Store::CorpusAgentPages),
        "a format folder swapped for a file"
    );
}

/// Failure: a folder that isn't there is read as an error (the removal is refused for a state that never had a cache) or as unlisted (it passes with the store
/// unmarked). Missing roots are complete and empty, and the older build's folder is still looked for.
#[test]
fn missing_roots_are_complete_and_empty() {
    let w = World::new();
    let seen = w.seen(TargetMode::Save);
    assert!(OURS.iter().all(|store| seen.complete(*store) && seen.count(*store) == 0));
    plain(&w.dir.join("corpus"));
    let seen = w.seen(TargetMode::Save);
    assert_eq!(seen.owner(Store::CorpusText, "old/corpus"), Owner::Unknown);
    assert!(seen.complete(Store::CorpusText), "complete: what was found is in the survey");
}

/// Failure: a cap isn't one (a folder of any size is read whole), or is one too soon (the limit itself is refused), or cuts the listing and goes on (a survey that
/// read only part reads as "none found"). Each cap, with it set small, passes at the limit and is `TooLarge` one past it.
#[test]
fn every_cap_holds_at_the_limit_and_one_past() {
    let refused = |w: &World, caps: &CacheCaps| w.survey(TargetMode::Save, caps, 4096).err();
    let strays = |w: &World, folder: &Path, n: usize| {
        let _ = w;
        for i in 0..n {
            plain(&folder.join(format!("stray{i}")));
        }
    };

    for (what, at_limit) in [("root entries", true), ("root entries", false)] {
        let w = World::new();
        strays(&w, &w.corpus, if at_limit { 3 } else { 4 });
        let caps = small(|c| c.root_entries = 3);
        assert_eq!(refused(&w, &caps), (!at_limit).then_some(Refusal::TooLarge), "{what} at the limit: {at_limit}");
    }
    for at_limit in [true, false] {
        let (w, mine, _) = two_documents();
        let key_dir = w.corpus.join(mine.hex());
        let have = fs::read_dir(&key_dir).unwrap().count();
        strays(&w, &key_dir, if at_limit { 1 } else { 2 });
        let caps = small(|c| c.key_entries = have + 1);
        assert_eq!(
            refused(&w, &caps),
            (!at_limit).then_some(Refusal::TooLarge),
            "key folder entries at the limit: {at_limit}"
        );
    }
    for at_limit in [true, false] {
        let (w, mine, _) = two_documents();
        let key_dir = w.corpus.join(mine.hex());
        if !at_limit {
            dir(&key_dir.join("v3"));
        }
        let caps = small(|c| c.subfolders = 1);
        assert_eq!(
            refused(&w, &caps),
            (!at_limit).then_some(Refusal::TooLarge),
            "format folders at the limit: {at_limit}"
        );
    }
    for at_limit in [true, false] {
        let (w, mine, _) = two_documents();
        let format_dir = w.corpus.join(mine.hex()).join("v4");
        let have = fs::read_dir(&format_dir).unwrap().count();
        strays(&w, &format_dir, if at_limit { 1 } else { 2 });
        let caps = small(|c| c.sub_entries = have + 1);
        assert_eq!(
            refused(&w, &caps),
            (!at_limit).then_some(Refusal::TooLarge),
            "format folder entries at the limit: {at_limit}"
        );
    }
    for at_limit in [true, false] {
        let w = World::new();
        for i in 0..if at_limit { 2 } else { 3 } {
            w.note(&hex_key(i + 1), hex_key(9).as_bytes());
        }
        let caps = small(|c| c.alias_entries = 2);
        assert_eq!(refused(&w, &caps), (!at_limit).then_some(Refusal::TooLarge), "notes at the limit: {at_limit}");
    }
    for at_limit in [true, false] {
        let w = World::new();
        strays(&w, &w.contents, if at_limit { 2 } else { 3 });
        let caps = small(|c| c.contents_entries = 2);
        assert_eq!(
            refused(&w, &caps),
            (!at_limit).then_some(Refusal::TooLarge),
            "contents at the limit: {at_limit}"
        );
    }
    // The keys a target's notes may name: its own and those of its notes.
    for at_limit in [true, false] {
        let w = World::new();
        let keys: Vec<String> = (1..=if at_limit { 2 } else { 3 }).map(hex_key).collect();
        w.note(&names::path_alias(w.mine.as_os_str().as_encoded_bytes()), keys.join("\n").as_bytes());
        let caps = small(|c| c.target_keys = 3);
        assert_eq!(
            refused(&w, &caps),
            (!at_limit).then_some(Refusal::TooLarge),
            "the target's keys at the limit: {at_limit}"
        );
    }
    // The survey's own cap passes through.
    for at_limit in [true, false] {
        let w = World::new();
        strays(&w, &w.corpus, if at_limit { 3 } else { 4 });
        let result = w.survey(TargetMode::Save, &CacheCaps::default(), 3).err();
        assert_eq!(result, (!at_limit).then_some(Refusal::TooLarge), "artifacts at the survey's limit: {at_limit}");
    }
}

/// Failure: a cut survey leaves a store marked, so the plan reads "none found" over a folder it never finished. Nothing is marked once a cap is passed.
#[test]
fn nothing_is_marked_when_a_cap_is_passed() {
    let w = World::new();
    for i in 0..4 {
        plain(&w.corpus.join(format!("stray{i}")));
    }
    let ledger = w.ledger();
    let lineage = lineage(&ledger, version(7), &w.mine, &LEDGER_LIMITS).unwrap();
    let mut survey = Survey::new(&limits(64));
    let caps = small(|c| c.root_entries = 3);
    assert_eq!(
        survey_caches_with(&w.corpus, &w.contents, &lineage, &w.target(), TargetMode::Save, &mut survey, &caps),
        Err(Refusal::TooLarge)
    );
    let seen = Seen::of(&survey.finish().unwrap());
    assert!(OURS.iter().all(|store| !seen.complete(*store)));
}

/// Failure: a note planted by another process is trusted past what the app's own clear would do, or a pending temp of the target's note is missed. A planted
/// note naming the target's own key under any name is the target's (it can only make the app clear more cache, which costs a rebuild), and one under no key
/// of the target's is not.
#[test]
fn a_planted_note_can_only_add_to_what_is_cleared() {
    let w = World::new();
    let mine = w.index(&w.mine);
    let other = hex_key(0x77);
    w.key_folder(&other);
    // Planted: a note under a made-up name that lists another document's folder along with the target's key.
    w.note(&"e".repeat(64), format!("{}\n{other}", mine.hex()).as_bytes());
    let seen = w.seen(TargetMode::Save);
    assert_eq!(seen.owner(Store::CorpusText, &mine.hex()), Owner::Target);
    // The other folder is the target's only if a note named for the target's path or inode lists it. This one doesn't.
    assert_eq!(seen.owner(Store::CorpusText, &other), Owner::Unknown);
    w.note(
        &names::path_alias(w.mine.as_os_str().as_encoded_bytes()),
        format!("{}\n{other}", mine.hex()).as_bytes(),
    );
    assert_eq!(
        w.seen(TargetMode::Save).owner(Store::CorpusText, &other),
        Owner::Target,
        "named for the target's path, it is the app's own rule"
    );
}
