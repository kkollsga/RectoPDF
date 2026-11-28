//! Blocks as commands see them. Each test names the failure it catches.

use super::*;
use crate::code::Code;
use crate::index::ReadOnlyCorpus;
use crate::{Condition, check_condition};
use recto_core::{DocFacts, OpenError, Toc};
use recto_index::corpus::{self, Begin, BlocksBuild, BuildLimits, build_text_corpus};
use recto_session::{DocLoader, Session};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

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

/// What the host would say of blocks: its state, what it was asked, and what the corpus folder looked like when it was told to stop.
struct Host {
    state: Mutex<GenState>,
    wants: AtomicU32,
    /// A build that doesn't end until it's told to.
    making: AtomicBool,
    /// After it's told to stop it still says it's making them this many looks more, as a helper does that takes a moment to end.
    still_stopping: AtomicU32,
    /// Whether the corpus folder was still there when `cancel_files` came.
    folder_when_cancelled: Mutex<Option<bool>>,
    folder: PathBuf,
}

impl Blocking for Host {
    fn want(&self, _ask: BlocksAsk) {
        self.wants.fetch_add(1, Ordering::SeqCst);
        *self.state.lock().unwrap() = GenState::Queued;
    }
    fn state(&self, _key: CorpusKey) -> GenState {
        self.state.lock().unwrap().clone()
    }
    fn cancel_files(&self, _paths: Option<&[PathBuf]>) {
        *self.folder_when_cancelled.lock().unwrap() = Some(self.folder.exists());
        if self.making.swap(false, Ordering::SeqCst) {
            self.still_stopping.store(3, Ordering::SeqCst);
        }
    }
    fn making(&self, _paths: Option<&[PathBuf]>) -> bool {
        self.making.load(Ordering::SeqCst)
            || self
                .still_stopping
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| left.checked_sub(1))
                .is_ok()
    }
}

/// A session with one tab on a real file whose index is finished, and a host for blocks.
struct World {
    session: Mutex<Session<Pdf>>,
    indexing: ReadOnlyCorpus,
    host: Host,
    file: recto_testpdf::TempPdf,
    key: CorpusKey,
    root: recto_testpdf::TempDir,
    find_key: FindKey,
}

impl SessionAccess for World {
    type Loader = Pdf;
    fn with<R>(&self, step: impl FnOnce(&mut Session<Pdf>) -> R) -> R {
        self.session.with(step)
    }
    fn indexing(&self) -> Option<&dyn crate::index::Indexing> {
        Some(&self.indexing)
    }
    fn blocking(&self) -> Option<&dyn Blocking> {
        Some(&self.host)
    }
}

fn a_world(indexed: bool) -> World {
    let file = recto_testpdf::TempPdf::new("doc.pdf", b"%PDF-1.7");
    let key = CorpusKey::of_file(&file.0).unwrap();
    let root = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-agent-blocks-");
    recto_platform::local::ensure_private_dir(&root).unwrap();
    let find_key = FindKey::for_tests(5);
    if indexed {
        let texts: Vec<Option<String>> = (0..3).map(|page| Some(format!("Page {page} has some words in it"))).collect();
        build_text_corpus(&find_key, &root, (key, Some(&file.0)), &texts).unwrap();
    }
    let session = Mutex::new(Session::new(Pdf));
    session.with(|session| session.open(&file.0).unwrap());
    World {
        session,
        indexing: ReadOnlyCorpus(root.to_path_buf(), Some(FindKey::for_tests(5))),
        host: Host {
            state: Mutex::new(GenState::Idle),
            wants: AtomicU32::new(0),
            making: AtomicBool::new(false),
            still_stopping: AtomicU32::new(0),
            folder_when_cancelled: Mutex::new(None),
            folder: root.join(key.hex()),
        },
        file,
        key,
        root,
        find_key,
    }
}

impl World {
    /// Blocks made for the three pages, as a finished build leaves them.
    fn make_blocks(&self) {
        let Begin::Building(mut build) = BlocksBuild::begin(&self.find_key, &self.root, self.key, &BuildLimits::default()) else {
            panic!("no build")
        };
        let mut chars = Vec::new();
        for page in 0..3 {
            build.text(page, &mut chars);
            build
                .put(&recto_index::blocks::PageBlocks {
                    page,
                    chars: chars.len() as u32,
                    blocks: Vec::new(),
                })
                .unwrap();
        }
        build.finish(&corpus::BlocksTally::default()).unwrap();
    }

    fn why(&self) -> &'static str {
        why_page(self, self.file.0.as_path())
    }

    fn wait(&self) -> ConditionState {
        check_condition(self, Condition::BlocksReady, &TabRef::Active, None)
    }
}

/// Failure: a `get` of a blocks grain starts a build on every ask (a helper a call), or none (so an agent is told `blocks_not_made` for ever), or says `blocks_building` for a file whose index isn't there, or
/// for one that was refused or failed. Asked once while nothing is on its way, `blocks_building` while it's queued or running, and `blocks_not_made` for everything else, asking nothing.
#[test]
fn get_says_blocks_building_while_queued_or_running_and_asks_once() {
    let world = a_world(true);
    assert_eq!(world.why(), BUILDING, "the first ask starts it");
    assert_eq!(world.host.wants.load(Ordering::SeqCst), 1);
    assert_eq!(world.why(), BUILDING);
    *world.host.state.lock().unwrap() = GenState::Running;
    assert_eq!(world.why(), BUILDING, "running");
    assert_eq!(world.host.wants.load(Ordering::SeqCst), 1, "asked once");
    for state in [GenState::Refused("too_large".into()), GenState::Failed("busy".into())] {
        *world.host.state.lock().unwrap() = state;
        assert_eq!(world.why(), NOT_MADE);
    }
    assert_eq!(world.host.wants.load(Ordering::SeqCst), 1, "a refusal or a failure isn't asked again");
    let bare = a_world(false);
    assert_eq!(bare.why(), NOT_MADE, "no index to make blocks from");
    assert_eq!(bare.host.wants.load(Ordering::SeqCst), 0, "nothing is asked of a file with no index");
}

/// Failure: `get` goes on saying blocks are building once they are made (until S5 reads them it says `blocks_not_made`), or asks for them again; or a stored set that doesn't add up is kept.
#[test]
fn made_blocks_are_not_asked_for_again() {
    let world = a_world(true);
    world.make_blocks();
    assert_eq!(world.why(), NOT_MADE);
    assert_eq!(world.host.wants.load(Ordering::SeqCst), 0);
    let header = world.root.join(world.key.hex()).join("v4/app/blocks.sealed");
    std::fs::write(&header, b"not a header").unwrap();
    assert_eq!(world.why(), BUILDING, "a header that doesn't open is thrown away and they're asked for again");
    assert!(!header.exists());
}

/// Failure: `index clear` takes the corpus's folder while a blocks build holds its lock and writes into it (the clear then says the folder is being built and keeps it, or the build writes into nothing). Blocks
/// are told to stop, and waited for, before the folder is cleared, for one file and for all; and what was being built isn't left counted.
#[test]
fn index_clear_stops_a_blocks_build_before_it_clears() {
    for all in [false, true] {
        let world = a_world(true);
        world.host.making.store(true, Ordering::SeqCst);
        let said = crate::index::clear(&world, &TabRef::Active, all).unwrap();
        assert_eq!(
            *world.host.folder_when_cancelled.lock().unwrap(),
            Some(true),
            "stopped after the folder was gone (all: {all})"
        );
        assert!(!world.root.join(world.key.hex()).exists(), "the folder is cleared (all: {all})");
        assert_eq!(said["being_built"], 0, "{said}");
        assert_eq!(
            world.host.still_stopping.load(Ordering::SeqCst),
            0,
            "the clear didn't wait for the build to end (all: {all})"
        );
    }
}

/// Failure: a wait for blocks ends wrong: a refusal waits out its time or isn't a refusal, a failure doesn't say to ask again, a wait without the index doesn't ask for it, blocks that are made give more than
/// numbers, or a damaged set is believed. Every state, and the words of every code.
#[test]
fn wait_blocks_ready_maps_each_state() {
    let world = a_world(true);
    assert_eq!(world.wait(), ConditionState::Waiting);
    assert_eq!(world.wait(), ConditionState::Waiting);
    assert_eq!(world.host.wants.load(Ordering::SeqCst), 1, "asked once");
    let failed = |state: ConditionState| match state {
        ConditionState::Failed(error) => error,
        other => panic!("{other:?}"),
    };
    *world.host.state.lock().unwrap() = GenState::Refused("needs_password".into());
    let error = failed(world.wait());
    assert_eq!((error.code, error.exit), (Code::BLOCKS_REFUSED, crate::exit::PROTECTED));
    *world.host.state.lock().unwrap() = GenState::Refused("too_large".into());
    let error = failed(world.wait());
    assert_eq!((error.code, error.exit), (Code::BLOCKS_REFUSED, crate::exit::FAILED));
    *world.host.state.lock().unwrap() = GenState::Failed("busy".into());
    let error = failed(world.wait());
    assert_eq!((error.code, error.exit), (Code::BLOCKS_FAILED, crate::exit::FAILED));
    assert!(error.message.contains("Ask again in a minute"), "{}", error.message);
    world.make_blocks();
    let ConditionState::Met(value) = world.wait() else {
        panic!("made blocks aren't met")
    };
    assert_eq!((value["state"].as_str(), value["pages"].as_u64()), (Some("ready"), Some(3)));
    assert!(
        value["bytes"].as_u64().is_some_and(|bytes| bytes > 0) && value["bytes_per_page"].as_u64().is_some_and(|bytes| bytes > 0),
        "{value}"
    );
    assert_eq!(value["by_kind"]["table"]["stored"], 0);
    assert!(
        value.as_object().unwrap().values().all(|entry| !entry.is_string() || entry == "ready"),
        "numbers only: {value}"
    );
    let header = world.root.join(world.key.hex()).join("v4/app/blocks.sealed");
    std::fs::write(&header, b"junk").unwrap();
    *world.host.state.lock().unwrap() = GenState::Idle;
    assert_eq!(world.wait(), ConditionState::Waiting, "a damaged set is thrown away and made again");
    assert!(!header.exists());
    // No host, and no index.
    let bare = a_world(false);
    // No index to make blocks from: the wait is the index's own, and asks nothing of blocks.
    assert_eq!(bare.wait(), ConditionState::Waiting, "an index that isn't made is waited for");
    assert_eq!(bare.host.wants.load(Ordering::SeqCst), 0);
    let fallback = reason_words("something new");
    for code in [
        "needs_password",
        "couldnt_read",
        "no_pages",
        "pages_differ",
        "too_large",
        "over_memory",
        "out_of_time",
        "helper_missing",
        "helper_stopped",
        "bad_answer",
        "structure_not_sandboxed",
        "structure_inside_another_sandbox",
        "structure_walls_failed",
        "structure_start_refused",
        "structure_unconfirmed",
        "busy",
        "index_not_ready",
        "index_damaged",
    ] {
        assert_ne!(reason_words(code), fallback, "{code} has no words");
    }
}
