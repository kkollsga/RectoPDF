//! `recto index clear` and `recto index --agent` while a removal holds the caches, over a real finished corpus and a real contents file. The doubles stand for
//! the indexer (a use of the caches that is refused, or counted) and the contents' host. Each test names the failure it catches.

use super::*;
use crate::generated::{GenState, Generating};
use crate::{SessionAccess, TabRef};
use recto_core::{DocFacts, OpenError, Toc};
use recto_index::corpus::build_text_corpus;
use recto_session::{DocLoader, Session};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Pdf;

impl DocLoader for Pdf {
    fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
        Ok(DocFacts {
            page_count: 3,
            byte_len: 8,
            encrypted: false,
            version: "1.7".into(),
            title: None,
            toc: Toc::default(),
            renderer_only: false,
        })
    }
}

/// Uses of the caches that are out.
#[derive(Default)]
struct Out(AtomicU32);

impl UseEnded for Out {
    fn use_ended(&self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// How the indexer double answers a use of the caches.
enum Uses {
    /// A removal holds the caches.
    Refused,
    /// Counted, as the real indexer's is.
    Counted,
}

struct Indexer {
    root: PathBuf,
    key: FindKey,
    uses: Uses,
    out: Out,
    /// What `made_agent_folder` answers.
    claim: bool,
    cancels: AtomicU32,
}

impl Indexing for Indexer {
    fn root(&self) -> &Path {
        &self.root
    }
    fn find_key(&self) -> Option<KeyUse<'_>> {
        Some(KeyUse::plain(&self.key))
    }
    fn cache_use(&self) -> Option<CacheUse<'_>> {
        match self.uses {
            Uses::Refused => None,
            Uses::Counted => {
                self.out.0.fetch_add(1, Ordering::SeqCst);
                Some(CacheUse::counted(&self.out))
            }
        }
    }
    fn made_agent_folder(&self, _key: CorpusKey) -> bool {
        self.claim
    }
    fn want(&self, _path: &Path, _key: CorpusKey) {}
    fn job(&self, _key: CorpusKey) -> JobState {
        JobState::Idle
    }
    fn load(&self) -> (u32, u32) {
        (0, 0)
    }
    fn cancel(&self, _key: Option<CorpusKey>) {
        self.cancels.fetch_add(1, Ordering::SeqCst);
    }
}

/// The contents' host: counts the asks to stop, and says how many uses of the caches were out at the first (the corpus's deletes are done by then).
struct Contents {
    root: PathBuf,
    cancels: AtomicU32,
    uses_out_at_cancel: Mutex<Vec<u32>>,
    indexer: Arc<Indexer>,
}

impl Generating for Contents {
    fn root(&self) -> &Path {
        &self.root
    }
    fn want(&self, _path: &Path, _key: CorpusKey, _pages: u32) {}
    fn state(&self, _key: CorpusKey) -> GenState {
        GenState::Idle
    }
    fn held(&self, _key: CorpusKey) -> Option<Arc<recto_index::generated::Generated>> {
        None
    }
    fn cancel(&self, _key: Option<CorpusKey>) {}
    fn cancel_files(&self, _paths: Option<&[PathBuf]>) {
        self.cancels.fetch_add(1, Ordering::SeqCst);
        self.uses_out_at_cancel.lock().unwrap().push(self.indexer.out.0.load(Ordering::SeqCst));
    }
    fn making(&self, _paths: Option<&[PathBuf]>) -> bool {
        false
    }
}

struct World {
    session: Mutex<Session<Pdf>>,
    indexer: Arc<Indexer>,
    contents: Contents,
    _file: recto_testpdf::TempPdf,
    key: CorpusKey,
    root: recto_testpdf::TempDir,
    contents_root: recto_testpdf::TempDir,
}

impl SessionAccess for World {
    type Loader = Pdf;
    fn with<R>(&self, step: impl FnOnce(&mut Session<Pdf>) -> R) -> R {
        self.session.with(step)
    }
    fn indexing(&self) -> Option<&dyn Indexing> {
        Some(&*self.indexer)
    }
    fn generating(&self) -> Option<&dyn Generating> {
        Some(&self.contents)
    }
}

fn a_world(uses: Uses, claim: bool) -> World {
    let file = recto_testpdf::TempPdf::new("doc.pdf", b"%PDF-1.7");
    let key = CorpusKey::of_file(&file.0).unwrap();
    let temp = std::env::temp_dir().canonicalize().unwrap();
    let root = recto_testpdf::TempDir::new_in(&temp, "recto-agent-clear-");
    recto_platform::local::ensure_private_dir(&root).unwrap();
    let contents_root = recto_testpdf::TempDir::new_in(&temp, "recto-agent-clear-contents-");
    let find_key = FindKey::for_tests(5);
    let texts: Vec<Option<String>> = (0..3).map(|page| Some(format!("Page {page} has some words in it"))).collect();
    build_text_corpus(&find_key, &root, (key, Some(&file.0)), &texts).unwrap();
    recto_index::generated::write(&contents_root, &file.0, key, &recto_index::generated::Generated::default()).unwrap();
    let session = Mutex::new(Session::new(Pdf));
    session.with(|session| session.open(&file.0).unwrap());
    let indexer = Arc::new(Indexer {
        root: root.to_path_buf(),
        key: FindKey::for_tests(5),
        uses,
        out: Out::default(),
        claim,
        cancels: AtomicU32::new(0),
    });
    let contents = Contents {
        root: contents_root.to_path_buf(),
        cancels: AtomicU32::new(0),
        uses_out_at_cancel: Mutex::new(Vec::new()),
        indexer: Arc::clone(&indexer),
    };
    World {
        session,
        indexer,
        contents,
        _file: file,
        key,
        root,
        contents_root,
    }
}

impl World {
    fn corpus_folder(&self) -> PathBuf {
        self.root.join(self.key.hex())
    }

    fn contents_files(&self) -> usize {
        std::fs::read_dir(&self.contents_root).unwrap().flatten().count()
    }
}

/// Failure: `recto index clear` runs while a removal holds the caches, so it deletes in the folders the walk has pinned (and cancels the builds the hold has stopped). Refused, for a tab and for `--all`: the answer is `REMOVAL_BUSY`, nothing is deleted, and nothing was asked to stop.
#[test]
fn a_clear_during_a_removal_deletes_nothing_and_says_so() {
    for all in [false, true] {
        let world = a_world(Uses::Refused, true);
        let error = clear(&world, &TabRef::Active, all).unwrap_err();
        assert_eq!((error.code, error.exit), (Code::REMOVAL_BUSY, exit::BUSY), "all: {all}");
        assert!(world.corpus_folder().exists(), "the corpus was deleted (all: {all})");
        assert_eq!(world.contents_files(), 1, "the contents were deleted (all: {all})");
        assert_eq!(world.indexer.cancels.load(Ordering::SeqCst), 0, "the indexer was asked to stop (all: {all})");
        assert_eq!(world.contents.cancels.load(Ordering::SeqCst), 0, "the contents were asked to stop (all: {all})");
    }
}

/// Failure: a clear lets go of its use of the caches before its last delete (the use taken and dropped on one line), so a hold is had while the contents are still being deleted. At the contents' clear, after the corpus's deletes, the use is still out: once for a tab's clear and for `--all`, and gone after.
#[test]
fn a_clears_use_is_out_until_its_last_delete() {
    for all in [false, true] {
        let world = a_world(Uses::Counted, true);
        clear(&world, &TabRef::Active, all).expect("cleared");
        assert!(!world.corpus_folder().exists() && world.contents_files() == 0, "it cleared (all: {all})");
        assert_eq!(
            *world.contents.uses_out_at_cancel.lock().unwrap(),
            vec![1],
            "the use was out at the contents' clear (all: {all})"
        );
        assert_eq!(world.indexer.out.0.load(Ordering::SeqCst), 0, "the use ended with the clear (all: {all})");
    }
}

/// Failure: an agent's folder is written although the claim for it was refused (a removal held the indexer between the key and the claim), leaving a folder with neither the mark nor a claim, which no sweep removes. Nothing is written, and the answer is `REMOVAL_BUSY`.
#[test]
fn an_agent_folder_whose_claim_is_refused_is_not_written() {
    let world = a_world(Uses::Counted, false);
    let error = index(&world, &TabRef::Active, false, true).unwrap_err();
    assert_eq!((error.code, error.exit), (Code::REMOVAL_BUSY, exit::BUSY));
    assert!(corpus::agent_folder_bytes(&world.root, world.key).is_none(), "the agent's folder was written");
    // With the claim, the same ask writes it: the refusal is the claim's, not the rig's.
    let free = a_world(Uses::Counted, true);
    index(&free, &TabRef::Active, false, true).expect("made");
    assert!(
        corpus::agent_folder_bytes(&free.root, free.key).is_some(),
        "the rig can't make the folder, so the test above proved nothing"
    );
}
